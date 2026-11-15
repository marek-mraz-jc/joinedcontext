"""NetworkPolicy peers say which namespace they mean, or they are not rendered at all.

A peer that carries only a `podSelector` is valid YAML, passes kubeconform and means
something quite specific to Kubernetes: pods with those labels *in the policy's own
namespace*. So a reference that fails to resolve does not blow up — it silently rewrites
the rule to something narrower than what was written, and only a broken connection between
two namespaces, months later, says so.
"""

import ipaddress
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
COMPONENTS = PROJECT_ROOT / "components"
ENVIRONMENTS = ("local", "production", "dev")

# The tree `just _dev-assemble` needs, plus the roots the assembled helmfile reaches back into.
TREE = ("components", "defaults", "ci", "scripts", "helmfile-root.yaml.gotmpl",
        "helmfile-components.yaml.gotmpl")


def peers(doc):
    """Every (policy name, direction, peer) in one NetworkPolicy document."""
    for direction, key in (("ingress", "from"), ("egress", "to")):
        for rule in (doc["spec"].get(direction) or []):
            yield direction, key, rule


def policy_files():
    return sorted(COMPONENTS.glob("*/networkpolicies*.yaml"))


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_no_peer_selects_pods_without_naming_its_namespace(rendered, env):
    offenders = []
    for doc in rendered(env):
        if doc.get("kind") != "NetworkPolicy":
            continue
        for direction, key, rule in peers(doc):
            for peer in (rule.get(key) or []):
                if "podSelector" in peer and "namespaceSelector" not in peer:
                    offenders.append(f"{doc['metadata']['name']}.{direction}: {peer['podSelector']}")
    assert not offenders, "peers that quietly mean 'this namespace only':\n" + "\n".join(offenders)


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_no_rule_lists_an_empty_peer_set(rendered, env):
    """An empty `from`/`to` is not "no sources" to Kubernetes, it is every source."""
    offenders = []
    for doc in rendered(env):
        if doc.get("kind") != "NetworkPolicy":
            continue
        for direction, key, rule in peers(doc):
            if key in rule and not rule[key]:
                offenders.append(f"{doc['metadata']['name']}.{direction}")
    assert not offenders, "rules that open to everything:\n" + "\n".join(offenders)


def test_every_component_namespace_reference_names_a_component_that_exists():
    """The render drops a peer whose component is absent from the environment, so a typo
    would look exactly like a component that is merely switched off. Catch it here."""
    unknown = []
    for path in policy_files():
        for policy in (yaml.safe_load(path.read_text()) or {}).values():
            refs = [policy.get("componentNamespace")]
            for _, key, rule in peers({"spec": policy}):
                refs += [p.get("componentNamespace") for p in (rule.get(key) or [])]
            for ref in filter(None, refs):
                component = ref.split(".")[0]
                if not (COMPONENTS / component).is_dir():
                    unknown.append(f"{path.relative_to(PROJECT_ROOT)}: {ref}")
    assert not unknown, "componentNamespace naming no component:\n" + "\n".join(unknown)


def test_a_policy_for_another_components_pods_is_pinned_to_their_namespace():
    """A policy lands in its own component's namespace unless it names another with a
    top-level `componentNamespace`, and its `podSelector` only ever selects pods there. So
    when two components each declare an unpinned policy for the same pods, one of them
    selects nothing in the multi-namespace variants, which is how the database refused the
    broker's and the Portal's meshed connections on 4143 (ci-full 36104814442, variant 1,1,1)
    while every single-namespace variant stayed green."""
    declared = {}
    for path in policy_files():
        component = path.parent.name
        for name, policy in (yaml.safe_load(path.read_text()) or {}).items():
            selector = policy.get("podSelector") or {}
            if not selector or policy.get("componentNamespace") or policy.get("namespace"):
                continue
            key = tuple(sorted(selector.items()))
            declared.setdefault(key, {}).setdefault(component, []).append(name)
    shared = {
        str(dict(key)): owners for key, owners in declared.items() if len(owners) > 1
    }
    assert not shared, (
        "pods selected by unpinned policies of more than one component; pin the foreign ones "
        f"with componentNamespace: {shared}"
    )


def test_the_broker_rule_names_the_gateway_peer_the_moment_the_component_is_deployed(rendered):
    """dev deploys the context-gateway since T-0263, so the broker's rule for it is rendered
    with both peers. While dev left the component out, this test was the proof that a peer
    naming an absent component is dropped rather than rendered empty; that half of the
    guard now lives only in the test below, which makes the render abort on the wrong case."""
    broker = [d for d in rendered("dev")
              if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "context-broker"]
    assert broker, "the dev render lost the context-broker policy entirely"
    selectors = [str(peer) for _, key, rule in peers(broker[0]) for peer in (rule.get(key) or [])]
    assert any("context-gateway" in s for s in selectors), selectors
    assert any("portal" in s for s in selectors), selectors


def test_a_deployed_component_that_resolves_to_no_namespace_aborts_the_render(tmp_path):
    """The other half of the same guard: dropping the peer is right only when the component
    is genuinely not here. A reference into a component that IS deployed and still resolves
    to nothing is a mistake, and the render has to stop rather than narrow the rule."""
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    for entry in TREE:
        source = PROJECT_ROOT / entry
        (shutil.copytree if source.is_dir() else shutil.copy)(source, tmp_path / entry)
    shutil.copytree(tmp_path / "defaults/deployment", tmp_path / "deployment")
    shutil.copy(tmp_path / "ci/example-deployments/helmfile.yaml", tmp_path / "deployment/helmfile.yaml")
    shutil.copytree(tmp_path / "ci/example-deployments/environments",
                    tmp_path / "deployment/environments", dirs_exist_ok=True)

    # A peer, not the policy-level key: an unresolvable policy namespace drops the whole
    # release, which is fail-closed and a different question. `portal` is deployed in local.
    broken = tmp_path / "components/context-broker/networkpolicies.yaml"
    text = broken.read_text()
    assert text.count("componentNamespace: portal\n") == 1, "fixture moved, pick another peer"
    broken.write_text(text.replace("componentNamespace: portal\n",
                                   "componentNamespace: portal.no-such-part\n"))

    result = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "local", "template",
         "--skip-deps", "-q", "--selector", "component=networkpolicies"],
        cwd=str(tmp_path), capture_output=True, text=True,
    )
    assert result.returncode != 0, "a dangling reference into a deployed component rendered fine"
    assert "resolves to no namespace" in result.stderr + result.stdout


def egress_rules_by_selector(docs):
    """Every egress rule in the render, grouped by the pods it applies to.

    A workload's egress is spread over several policies on purpose — the plain one next to
    the component, the mesh one that only exists when Linkerd is on — and Kubernetes unions
    them, so the group is the unit worth asserting about. Grouping on the exact selector is
    deliberate: two egress policies written for the same workload have to agree on which
    pods they cover, or one of them is quietly governing a different set."""
    groups: dict[frozenset, list] = {}
    for doc in docs:
        if doc.get("kind") != "NetworkPolicy" or "Egress" not in (doc["spec"].get("policyTypes") or []):
            continue
        key = frozenset(((doc["spec"].get("podSelector") or {}).get("matchLabels") or {}).items())
        groups.setdefault(key, []).extend(doc["spec"].get("egress") or [])
    return groups


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_default_deny_closes_both_directions(rendered, env):
    """Ingress-only default-deny leaves a compromised pod free to reach the database, the
    identity provider or an address on the internet (OPS-38, SEC-GAP-05)."""
    denies = [d for d in rendered(env)
              if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"].startswith("default-deny-")]
    assert denies, "no default-deny policy in the render at all"
    for doc in denies:
        spec = doc["spec"]
        assert spec.get("policyTypes") == ["Ingress", "Egress"], doc["metadata"]["name"]
        assert spec.get("podSelector") == {}, f"{doc['metadata']['name']} does not cover every pod"
        assert not spec.get("ingress") and not spec.get("egress"), \
            f"{doc['metadata']['name']} carries rules — a default-deny allows nothing"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_every_workload_under_an_egress_policy_can_still_resolve_dns(rendered, env):
    """The first thing egress default-deny breaks, and the least obvious: without CoreDNS
    every rule below it is dead too, because they all name Services."""
    missing = []
    for selector, rules in egress_rules_by_selector(rendered(env)).items():
        if not selector:
            continue  # the default-deny itself
        if not rules:
            continue  # a workload that calls nothing (node-exporter, T-5154): no DNS to keep
        resolves = any(
            peer.get("podSelector", {}).get("matchLabels", {}).get("k8s-app") == "kube-dns"
            and any(p["port"] == 53 for p in (rule.get("ports") or []))
            for rule in rules for peer in (rule.get("to") or [])
        )
        if not resolves:
            missing.append(dict(selector))
    assert not missing, f"egress policies with no route to CoreDNS: {missing}"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_no_egress_rule_opens_a_wildcard_destination(rendered, env):
    """Explicit allow-lists only: a peer says which namespace, and an address range says
    which ports. `namespaceSelector: {}` is every namespace; a portless ipBlock is every
    port on the internet."""
    offenders = []
    for doc in rendered(env):
        if doc.get("kind") != "NetworkPolicy":
            continue
        for rule in (doc["spec"].get("egress") or []):
            for peer in (rule.get("to") or []):
                where = doc["metadata"]["name"]
                if peer.get("namespaceSelector") == {}:
                    offenders.append(f"{where}: namespaceSelector {{}} — every namespace")
                if "ipBlock" in peer and not rule.get("ports"):
                    offenders.append(f"{where}: {peer['ipBlock']['cidr']} on every port")
    assert not offenders, "\n".join(offenders)


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_every_policy_fits_a_helm_release_name(rendered, env):
    """One release per policy, named `networkpolicies-<policy>`. Helm rejects anything over
    53 characters, and it does it at sync time, long after the render looked fine."""
    too_long = [n for n in (d["metadata"]["name"] for d in rendered(env)
                            if d.get("kind") == "NetworkPolicy")
                if len(f"networkpolicies-{n}") > 53]
    assert not too_long, f"policy names that helm will refuse: {too_long}"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_keycloak_accepts_the_gateway_without_the_mesh(rendered, env):
    """The gateway fetches the realm's JWKS over the in-cluster Service at start-up and exits
    when it cannot. The Linkerd mirror lets it in on 4143; this is the plain 8080 path the
    unmeshed k3d variants take, which crash-looped the gateway in ci-full three runs long."""
    docs = [d for d in rendered(env) if d.get("kind") == "NetworkPolicy"
            and d["spec"].get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/instance") == "keycloak-app"
            and "Ingress" in d["spec"].get("policyTypes", [])
            and not d["metadata"]["name"].endswith("linkerd")]
    if not docs:
        pytest.skip("keycloak not deployed in this environment")
    peers = [peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name")
             for d in docs for rule in d["spec"].get("ingress") or [] for peer in rule.get("from") or []]
    assert "context-gateway-gateway" in peers, peers
    # F-R2DEP2-002: every rule names its ports; a rule without them opens JGroups (7800, 57800).
    for d in docs:
        for rule in d["spec"].get("ingress") or []:
            assert rule.get("ports"), (d["metadata"]["name"], rule)
            names = {peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name") for peer in rule.get("from") or []}
            if d["metadata"]["name"] == "keycloak" and "keycloakx" not in names:
                assert {p["port"] for p in rule["ports"]} == {8080}, (d["metadata"]["name"], rule)


MESH_PORTS = {4143, 4190, 4191}
CONTROL_PLANE_PORTS = {8080, 8086, 8090}
INFRA_PORTS = MESH_PORTS | CONTROL_PLANE_PORTS | {53}


# Peers that answer on their service port without the mesh, so an egress rule to them needs no
# companion 4143 rule. Each one is a workload the mesh does not inject, not a workload whose
# companion rule was forgotten: the difference is the whole point of the test below.
UNMESHED_PEERS: list[dict] = [
    # The mail catcher stays outside the mesh: SMTP speaks first, and it serves no platform call
    # (T-3583); the Portal reaches it on 1025 itself.
    {"app.kubernetes.io/name": "mail-catcher"},
]


@pytest.mark.parametrize("env", ["dev"])
def test_an_unmeshed_peer_opts_out_of_the_namespace_s_injection(rendered, env):
    """The namespace injects every pod, so an unmeshed workload must say `disabled`: left
    unannotated it is meshed anyway, its proxy has no egress to the control plane, the startup
    probe gets a 403 and the rollout never finishes (mail-catcher on dev, 2026-10-11)."""
    found = []
    for doc in rendered(env):
        if doc.get("kind") not in ("Deployment", "StatefulSet", "DaemonSet"):
            continue
        template = doc["spec"]["template"]
        labels = template["metadata"].get("labels") or {}
        if any(peer.items() <= labels.items() for peer in UNMESHED_PEERS):
            found.append(doc["metadata"]["name"])
            annotations = template["metadata"].get("annotations") or {}
            assert annotations.get("linkerd.io/inject") == "disabled", doc["metadata"]["name"]
            # Admission refuses an opt-out without its reason (autogen-opt-out-needs-reason).
            assert (annotations.get("mesh.joinedcontext.com/opt-out-reason") or "").strip(), \
                doc["metadata"]["name"]
    assert found, "no unmeshed workload in the render: UNMESHED_PEERS names nothing deployed"


@pytest.mark.parametrize("env", ["dev"])
def test_a_meshed_egress_to_a_pod_peer_also_allows_that_peer_s_inbound_proxy(rendered, env):
    """Meshed traffic lands on the peer's Linkerd inbound proxy on 4143, not on the service
    port, so an egress rule naming only the service port allows a connection the proxy never
    makes. The pod resolves the peer, opens a connection that dies with "server closed the
    connection unexpectedly", and retries until something upstream gives up.

    That is what `database-role-setup` did: it allowed 5432 to the CNPG pods and nothing on
    4143, so the CKAN DataStore role Job could never reach the database and the post-upgrade
    hook timed out, failing the whole release. Every other meshed workload here already carries
    the companion rule; this asserts none is forgotten again.
    """
    policies = [d for d in rendered(env) if d.get("kind") == "NetworkPolicy"]

    def peer_key(peer):
        """One peer, as the pair of selectors that identifies it in a rule."""
        return repr(
            (
                sorted((peer.get("podSelector", {}).get("matchLabels") or {}).items()),
                sorted((peer.get("namespaceSelector", {}).get("matchLabels") or {}).items()),
            )
        )

    # Which peers does each policy's subject get 4143 egress to? Keyed by the peer itself, not
    # by "any peer": a pod that reaches the gateway's proxy is not thereby able to reach the
    # store's, which is exactly how the reconciler lost the artifact store (T-0933).
    mesh_reach: dict[str, set[str]] = {}
    for policy in policies:
        subject = policy["spec"].get("podSelector", {}).get("matchLabels") or {}
        if not subject:
            continue
        for rule in policy["spec"].get("egress", []):
            ports = {p.get("port") for p in rule.get("ports", [])}
            if 4143 not in ports:
                continue
            reached = mesh_reach.setdefault(repr(sorted(subject.items())), set())
            reached.update(peer_key(peer) for peer in rule.get("to", []))

    failures = []
    for policy in policies:
        spec = policy["spec"]
        subject = spec.get("podSelector", {}).get("matchLabels") or {}
        if not subject:
            continue
        key = repr(sorted(subject.items()))
        for rule in spec.get("egress", []):
            ports = {p.get("port") for p in rule.get("ports", [])}
            if not ports or ports <= INFRA_PORTS:
                continue
            # An egress to a pod peer inside the cluster, on a service port.
            peers = [t for t in rule.get("to", []) if "podSelector" in t]
            if not peers:
                continue
            for peer in peers:
                labels = peer["podSelector"].get("matchLabels") or {}
                # A peer with no labels selects a whole namespace rather than one workload —
                # an egress to something outside the mesh's reach, which needs no companion.
                if not labels or labels in UNMESHED_PEERS:
                    continue
                if peer_key(peer) not in mesh_reach.get(key, set()):
                    failures.append(
                        f"{policy['metadata']['name']} lets {subject} reach {labels} on "
                        f"{sorted(ports)} and nothing gives it 4143 to that peer"
                    )
    assert not failures, "meshed egress without the peer's inbound proxy:\n" + "\n".join(failures)


# The ranges a delivery must never reach: the pod and service networks, the node's own
# addresses, and the link-local address every cloud puts its metadata service on.
PRIVATE_RANGES = {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8", "100.64.0.0/10"}
# No public feed lives here either, and a typed DataSource URL must not reach a node on a CGNAT
# overlay or a multicast group (F-BNT3-004).
RESERVED_RANGES = {"100.64.0.0/10", "0.0.0.0/8", "198.18.0.0/15", "224.0.0.0/4"}


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_gateway_dials_nothing_outside_the_cluster_and_takes_nothing_from_the_broker(rendered, env):
    """T-3925: the broker delivers a notification itself (plan 5.5), so the gateway keeps no route
    to a subscriber and no token-less delivery path: no ipBlock in its egress, and the broker is
    no peer of its ingress, plain or meshed."""
    docs = rendered(env)
    groups = egress_rules_by_selector(docs)
    gateway = next((rules for selector, rules in groups.items()
                    if dict(selector).get("app.kubernetes.io/name") == "context-gateway-gateway"),
                   None)
    if gateway is None:
        pytest.skip("context-gateway not deployed in this environment")
    assert not [rule for rule in gateway if any("ipBlock" in peer for peer in rule.get("to") or [])]
    names = {d["metadata"]["name"] for d in docs if d.get("kind") == "NetworkPolicy"}
    assert "context-gateway-notification-egress" not in names
    for doc in docs:
        if doc.get("kind") != "NetworkPolicy":
            continue
        spec = doc["spec"]
        if (spec.get("podSelector") or {}).get("matchLabels", {}).get("app.kubernetes.io/name") != "context-gateway-gateway":
            continue
        peers = [peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name")
                 for rule in spec.get("ingress") or [] for peer in rule.get("from") or []]
        assert "context-broker-broker" not in peers, doc["metadata"]["name"]


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_broker_delivers_to_public_addresses_only(rendered, env):
    """Once a subscription is stored through a broker view (T-3653), the broker delivers the
    notification itself, to the address the subscription names. The allowance is a range with
    every private range excepted, so a subscription cannot steer the broker at the cluster, the
    node or a metadata service (T-3882, G11)."""
    groups = egress_rules_by_selector(rendered(env))
    broker = next((rules for selector, rules in groups.items()
                   if dict(selector).get("app.kubernetes.io/name") == "context-broker-broker"),
                  None)
    if broker is None:
        pytest.skip("context-broker not deployed in this environment")
    delivery = [rule for rule in broker if any("ipBlock" in peer for peer in rule.get("to") or [])]
    assert delivery, "the broker has no route to a subscriber at all"
    for rule in delivery:
        assert {p["port"] for p in rule.get("ports") or []} == {80, 443}, rule.get("ports")
        for peer in rule["to"]:
            reached = set(peer["ipBlock"].get("except") or [])
            assert reached >= PRIVATE_RANGES, f"reaches {PRIVATE_RANGES - reached}"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_pipeline_runner_reaches_public_addresses_and_the_ingress_controller_only(rendered, env):
    """A check probes the URL of a DataSource a person typed on the runner (MF-39), so what the
    runner may reach over HTTPS is what that URL may reach: every public address, and inside the
    cluster only the ingress controller the public host is rewritten to. The Kubernetes API, a
    webhook, Keycloak's own port and a metadata service are none of them (T-0752)."""
    groups = egress_rules_by_selector(rendered(env))
    runner = next((rules for selector, rules in groups.items()
                   if dict(selector).get("app.kubernetes.io/name") == "pipeline-runner-runner"), None)
    if runner is None:
        pytest.skip("pipeline-runner not deployed in this environment")
    ranges = [rule for rule in runner if any("ipBlock" in peer for peer in rule.get("to") or [])]
    assert ranges, "the runner reaches no feed at all"
    for rule in ranges:
        for peer in rule["to"]:
            excepted = set(peer["ipBlock"].get("except") or [])
            assert excepted >= PRIVATE_RANGES | RESERVED_RANGES, f"the runner reaches {(PRIVATE_RANGES | RESERVED_RANGES) - excepted}"
    https = {443, 8443}
    for rule in runner:
        if not https & {p.get("port") for p in rule.get("ports") or []}:
            continue
        for peer in rule.get("to") or []:
            if "ipBlock" in peer:
                continue
            names = {value for expression in peer.get("podSelector", {}).get("matchExpressions", [])
                     for value in expression["values"]}
            names |= set(peer.get("podSelector", {}).get("matchLabels", {}).values())
            assert names <= {"traefik", "ingress-nginx"}, f"the runner reaches {names} over HTTPS"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_portal_reaches_the_api_server_on_the_port_the_service_dnats_to(rendered, env):
    """The reconciler writes to the cluster — an App's objects, the artifact store's reader
    Secret, a pipeline's credentials — through `kubernetes.default.svc:443`. kube-proxy rewrites
    that to the node's 6443 before the egress filter runs, so 443 alone leaves every one of those
    writes refused inside the pod, with `cannot reach the API server` as the only trace (T-0938).
    """
    groups = egress_rules_by_selector(rendered(env))
    portal = next((rules for selector, rules in groups.items()
                   if dict(selector).get("app.kubernetes.io/name") == "portal-portal"), None)
    if portal is None:
        pytest.skip("portal not deployed in this environment")
    reachable = {port.get("port")
                 for rule in portal
                 for peer in rule.get("to") or []
                 if "ipBlock" in peer
                 for port in rule.get("ports") or []}
    assert 6443 in reachable, f"the portal reaches {sorted(p for p in reachable if p)} and not the API server"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_portal_s_internal_listener_answers_the_runners_and_the_gateway_only(rendered, env):
    """Port 9090 carries the agent proxy's callbacks, a pipeline test's capture and the list of
    running workspace previews (Architecture/06 §7.2). None of them is on the edge, and the
    preview list carries no token, so who reaches the port is the whole of its access rule."""
    docs = [d for d in rendered(env) if d.get("kind") == "NetworkPolicy"
            and (d["spec"].get("podSelector") or {}).get("matchLabels", {}).get("app.kubernetes.io/name") == "portal-portal"]
    if not docs:
        pytest.skip("portal not deployed in this environment")
    reaching = set()
    for doc in docs:
        for rule in doc["spec"].get("ingress") or []:
            if 9090 not in [p.get("port") for p in rule.get("ports") or []]:
                continue
            for peer in rule["from"]:
                reaching.add(peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name"))
    assert reaching <= {"agent-runner-proxy", "pipeline-runner-runner", "context-gateway-gateway"}, reaching
    names = {d["metadata"]["name"] for d in rendered(env) if d.get("kind") == "NetworkPolicy"}
    if "context-gateway" in names:
        assert "context-gateway-gateway" in reaching


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_a_workload_that_mints_its_own_token_may_reach_the_realm(rendered, env):
    """A pod told to mint tokens at the in-cluster realm must have the egress rule to reach it.

    `JC_OIDC_TOKEN_URL` is the ClusterIP Service and not the public host, because a pod cannot
    dial this node's own public address. The credential proxy was configured that way and its
    policy allowed 443 to the world and nothing to Keycloak, so every grant it asked for was
    dropped on the way out. The proxy reports that as a transport failure, answers the run
    `401 invalid run credentials`, and the assistant's first question dies with it — a whole
    feature dark, from a rule nobody wrote (T-2420).
    """
    docs = rendered(env)
    keycloak = {"app.kubernetes.io/instance": "keycloak-app"}

    def reaches_keycloak(labels):
        for doc in docs:
            if doc.get("kind") != "NetworkPolicy":
                continue
            if doc["spec"].get("podSelector", {}).get("matchLabels") != labels:
                continue
            for rule in doc["spec"].get("egress") or []:
                for peer in rule.get("to") or []:
                    if (peer.get("podSelector", {}).get("matchLabels") or {}) == keycloak:
                        return True
        return False

    missing = []
    for doc in docs:
        if doc.get("kind") not in ("Deployment", "StatefulSet", "DaemonSet"):
            continue
        spec = doc["spec"]["template"]["spec"]
        for container in spec.get("containers") or []:
            for var in container.get("env") or []:
                # The Portal's tokens and admin calls (T-4450) are the same dial.
                if var.get("name") not in ("JC_OIDC_TOKEN_URL", "JC_KEYCLOAK_ADMIN_URL"):
                    continue
                if "svc.cluster.local" not in (var.get("value") or ""):
                    continue
                labels = doc["spec"]["template"]["metadata"].get("labels") or {}
                selector = {k: labels[k] for k in ("app.kubernetes.io/name",) if k in labels}
                if selector and not reaches_keycloak(selector):
                    missing.append(doc["metadata"]["name"])
    assert not missing, f"told to mint in-cluster, no egress to the realm: {sorted(set(missing))}"


METADATA = ipaddress.ip_address("169.254.169.254")
# The port cloud metadata services answer on (Hetzner, AWS, GCP and Azure alike).
METADATA_PORTS = {80}


def _reaches_metadata(peer):
    block = peer.get("ipBlock")
    if not block:
        return False
    if METADATA not in ipaddress.ip_network(block["cidr"]):
        return False
    return not any(METADATA in ipaddress.ip_network(cidr) for cidr in block.get("except") or [])


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_no_workload_reaches_the_metadata_service(rendered, env):
    """T-1710 (a compromised pod moves sideways): the metadata address hands out the node's
    identity and the cloud's user data. An egress rule to 0.0.0.0/0 reaches it unless it
    excludes the address or leaves out the metadata port. A rule without `ports` opens every
    port, 80 included."""
    offenders = []
    for doc in rendered(env):
        if doc.get("kind") != "NetworkPolicy":
            continue
        for rule in doc["spec"].get("egress") or []:
            if not any(_reaches_metadata(peer) for peer in rule.get("to") or []):
                continue
            ports = rule.get("ports")
            opened = {p.get("port") for p in ports} if ports else None
            if opened is None or METADATA_PORTS & opened or any(
                p.get("endPort") and p["port"] <= 80 <= p["endPort"] for p in ports or []
            ):
                offenders.append(f"{doc['metadata'].get('namespace')}/{doc['metadata']['name']}: {opened}")
    assert not offenders, "egress that reaches 169.254.169.254:80:\n" + "\n".join(offenders)


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_actions_runner_reaches_the_forge_the_portal_and_dns_only(rendered, env):
    """The runner runs an application's untrusted build code (T-1707, ADR-N-028 §5, AP-81): it
    takes jobs from the forge, proposes `status.build` to the Portal, and resolves names.
    Nothing else is reachable: no address range, no Kubernetes API, no Keycloak, no store."""
    groups = egress_rules_by_selector(rendered(env))
    runner = [rules for selector, rules in groups.items()
              if dict(selector).get("app.kubernetes.io/name") == "gitea-runner"]
    if not runner:
        pytest.skip("gitea-runner not deployed in this environment")
    reached = set()
    for rules in runner:
        for rule in rules:
            for peer in rule.get("to") or []:
                assert "ipBlock" not in peer, f"the runner reaches {peer['ipBlock']}"
                labels = peer.get("podSelector", {}).get("matchLabels", {})
                reached |= set(labels.values())
                for expression in peer.get("podSelector", {}).get("matchExpressions", []):
                    reached |= set(expression["values"])
    assert reached - {"gitea", "gitea-forge", "portal-portal", "kube-dns"} == set(), f"the runner reaches {reached}"



def _gateway_admits(policies, source: dict, port: int) -> bool:
    """Whether one of the gateway's ingress policies names `source` on `port`."""
    for policy in policies:
        spec = policy["spec"]
        selected = (spec.get("podSelector") or {}).get("matchLabels") or {}
        if selected.get("app.kubernetes.io/name") != "context-gateway-gateway":
            continue
        for rule in spec.get("ingress") or []:
            if port not in {p.get("port") for p in rule.get("ports") or []}:
                continue
            for peer in rule.get("from") or []:
                labels = (peer.get("podSelector") or {}).get("matchLabels") or {}
                if labels and set(labels.items()) <= set(source.items()):
                    return True
    return False


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_gateway_admits_the_portal_s_calls(rendered, env):
    """The Portal calls the gateway in the cluster (`JC_PORTAL_GATEWAY_URL`): the AP-132 access
    check before every App build, Subscriptions (T-0931), the assistant's reads. Its egress
    named the gateway on 8080 and, meshed, on 4143, and the gateway's ingress named the Portal
    on neither, so every build was refused with the Portal proxy's 504 (T-2944). Both halves."""
    policies = [d for d in rendered(env) if d.get("kind") == "NetworkPolicy"]
    portal = {"app.kubernetes.io/name": "portal-portal"}
    assert _gateway_admits(policies, portal, 8080), "the gateway's ingress refuses the Portal on 8080"
    if env == "dev":
        assert _gateway_admits(policies, portal, 4143), (
            "the gateway's inbound proxy (4143) refuses the Portal's meshed calls"
        )


PROMETHEUS = {"app.kubernetes.io/name": "prometheus"}
SCRAPED = ("context-broker-broker", "context-gateway-gateway", "portal-portal", "postgres-cluster")


def _prometheus_ingress(docs):
    """(policy, selected pods, peer, ports) for every ingress peer naming Prometheus' pods."""
    for doc in docs:
        if doc.get("kind") != "NetworkPolicy":
            continue
        labels = doc["spec"].get("podSelector", {}).get("matchLabels", {})
        pods = labels.get("app.kubernetes.io/name") or labels.get("cnpg.io/cluster")
        for rule in doc["spec"].get("ingress") or []:
            for peer in rule.get("from") or []:
                if peer.get("podSelector", {}).get("matchLabels") == PROMETHEUS:
                    yield doc["metadata"]["name"], pods, peer, sorted(p["port"] for p in rule["ports"])


def test_prometheus_is_admitted_to_the_scraped_workloads_from_its_own_namespace_alone(rendered):
    """T-3771: production names the operator's namespace, so the broker, the gateway and the
    Portal admit Prometheus' pods from there, and nowhere else: a label is anybody's to set."""
    found = list(_prometheus_ingress(rendered("production")))
    assert {pods for _, pods, _, _ in found} == set(SCRAPED), found
    for name, _, peer, _ in found:
        assert peer.get("namespaceSelector") == {
            "matchLabels": {"kubernetes.io/metadata.name": "monitoring"}
        }, (name, peer)
    ports = {(name, tuple(p)) for name, _, _, p in found}
    # T-5095: the broker's metrics port alone; 9090 is the API and its admin surface.
    assert ("context-broker", (9464,)) in ports, ports
    assert not [p for name, pods, _, p in found if (pods or "").startswith("context-broker") and 9090 in p], found
    assert ("context-gateway", (8080,)) in ports, ports
    assert ("postgres", (9187,)) in ports, ports  # the CloudNativePG exporter (T-5125)


# dev deploys the prometheus component since T-5154 (tests/test_prometheus_stack.py holds its rules).
@pytest.mark.parametrize("env", ("local",))
def test_without_a_prometheus_namespace_no_rule_admits_prometheus(rendered, env):
    """An empty monitoring.prometheusNamespace renders no peer and no rule, never one that
    falls back to the policy's own namespace or to every namespace."""
    assert not list(_prometheus_ingress(rendered(env)))


def test_every_namespace_from_reference_names_a_value_that_exists():
    """namespaceFrom: component.part.key, the key declared in that component's defaults, so a
    typo fails here and not only in the render of an environment that deploys the component."""
    unknown = []
    for path in policy_files():
        for policy in (yaml.safe_load(path.read_text()) or {}).values():
            for _, key, rule in peers({"spec": policy}):
                for peer in rule.get(key) or []:
                    ref = peer.get("namespaceFrom")
                    if not ref:
                        continue
                    component, *_, leaf = ref.split(".")
                    defaults = COMPONENTS / component / "default-environment.yaml.gotmpl"
                    if not defaults.is_file() or f"\n    {leaf}:" not in defaults.read_text():
                        unknown.append(f"{path.relative_to(PROJECT_ROOT)}: {ref}")
    assert not unknown, "namespaceFrom naming no value:\n" + "\n".join(unknown)


def test_a_namespace_from_typo_in_a_deployed_component_aborts_the_render(tmp_path):
    """The same guard as componentNamespace: monitoring is deployed in local, so a key that is
    not there is a typo and stops the render instead of reading as "not configured"."""
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    for entry in TREE:
        source = PROJECT_ROOT / entry
        (shutil.copytree if source.is_dir() else shutil.copy)(source, tmp_path / entry)
    shutil.copytree(tmp_path / "defaults/deployment", tmp_path / "deployment")
    shutil.copy(tmp_path / "ci/example-deployments/helmfile.yaml", tmp_path / "deployment/helmfile.yaml")
    shutil.copytree(tmp_path / "ci/example-deployments/environments",
                    tmp_path / "deployment/environments", dirs_exist_ok=True)
    broken = tmp_path / "components/context-broker/networkpolicies.yaml"
    text = broken.read_text()
    ref = "namespaceFrom: monitoring.monitoring.prometheusNamespace\n"
    assert text.count(ref) == 1, "fixture moved, pick another peer"
    broken.write_text(text.replace(ref, "namespaceFrom: monitoring.monitoring.prometheusNamespce\n"))

    result = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "local", "template",
         "--skip-deps", "-q", "--selector", "component=networkpolicies"],
        cwd=str(tmp_path), capture_output=True, text=True,
    )
    assert result.returncode != 0, "a namespaceFrom typo into a deployed component rendered fine"
    assert "names no value" in result.stderr + result.stdout


def _token_service_api_cidrs(docs):
    groups = egress_rules_by_selector(docs)
    tokens = next((rules for selector, rules in groups.items()
                   if dict(selector).get("app.kubernetes.io/name") == "pipeline-runner-tokens"), None)
    assert tokens is not None, "the token service has no egress policy"
    return sorted(peer["ipBlock"]["cidr"]
                  for rule in tokens
                  if 6443 in {p.get("port") for p in rule.get("ports") or []}
                  for peer in rule.get("to") or [] if "ipBlock" in peer)


@pytest.mark.parametrize(("env", "cidrs"), [("dev", ["10.255.0.101/32"]), ("smartcity", ["10.255.0.1/32"])])
def test_the_token_service_reaches_the_api_server_at_the_environment_s_addresses(rendered, env, cidrs):
    """The token service asks the Kubernetes API for a pipeline's ServiceAccount token on 6443.
    `global.kubeApiCidrs` names the API server's addresses where an environment knows them
    (smartcity), so the rule opens 6443 there only; the default keeps it open (F-BNT3-003)."""
    assert _token_service_api_cidrs(rendered(env)) == cidrs


@pytest.mark.parametrize(("env", "cidrs"), [("dev", ["10.255.0.101/32"]), ("smartcity", ["10.255.0.1/32"]),
                                            ("local", ["0.0.0.0/0"])])
def test_the_dr_drill_reaches_only_the_api_server(rendered, env, cidrs):
    """F-R2DEP3-009: the drill's token may exec into the primary as superuser, and its egress
    opened every internet host on 443. It reaches the API server's addresses on 6443 alone."""
    groups = egress_rules_by_selector(rendered(env))
    drill = next(rules for selector, rules in groups.items()
                 if dict(selector).get("app.kubernetes.io/component") == "database-dr-drill")
    blocks = [(peer["ipBlock"]["cidr"], sorted(p["port"] for p in rule.get("ports") or []))
              for rule in drill for peer in rule.get("to") or [] if "ipBlock" in peer]
    assert blocks == [(cidr, [6443]) for cidr in cidrs]


def _only_the_ingress_controller(peer):
    names = {value for expression in peer.get("podSelector", {}).get("matchExpressions", [])
             for value in expression["values"]}
    return "ipBlock" not in peer and names and names <= {"traefik", "ingress-nginx"}


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_the_internet_opens_only_where_a_workload_calls_it(rendered, env):
    """T-6218 (R2DEP1 F-004, F-005, F-016, F-017): the edge reaches the public issuer through the
    ingress controller the host is DNATed to, and nothing else on the internet; the ACME solver is
    called by that controller alone; the agent proxy's 8443 is that controller's port, not the
    internet's; apps-db opens 443 only to archive to an external store, which none of these
    environments does (dev archives to the in-cluster store)."""
    docs = [d for d in rendered(env) if d.get("kind") == "NetworkPolicy"]
    groups = egress_rules_by_selector(docs)

    def egress(name):
        return [rule for selector, rules in groups.items() if dict(selector).get("app.kubernetes.io/name") == name
                for rule in rules]

    edge = egress("apisix")
    assert edge, "apisix has no egress policy"
    assert not [r for r in edge if any("ipBlock" in p for p in r.get("to") or [])], "the edge reaches the internet"
    issuer = [r for r in edge if {443, 8443} <= {p["port"] for p in r.get("ports") or []}]
    assert issuer and all(_only_the_ingress_controller(p) for r in issuer for p in r["to"])

    (solver,) = [d for d in docs if d["metadata"]["name"] == "acme-http01-solver"]
    assert all(_only_the_ingress_controller(p) for r in solver["spec"]["ingress"] for p in r["from"])

    proxy = egress("agent-runner-proxy")
    assert proxy or env != "dev", "the agent proxy has no egress policy"
    for rule in proxy:
        if 8443 in {p["port"] for p in rule.get("ports") or []}:
            assert all(_only_the_ingress_controller(p) for p in rule["to"]), rule

    apps_db = [rule for selector, rules in groups.items() if dict(selector).get("cnpg.io/cluster") == "apps-db"
               for rule in rules]
    for rule in apps_db:
        if any("ipBlock" in p for p in rule.get("to") or []):
            assert 443 not in {p["port"] for p in rule.get("ports") or []}, f"apps-db reaches the internet on 443 in {env}"
