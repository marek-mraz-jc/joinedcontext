"""CC-01: the context broker reads no configuration of this plane: no repository, no manifest, no
ConfigMap of ours; what it is told arrives as its own environment, its own admin credential and its
database. The image is the jc build (surfaces and the joinedcontext engine, T-3642); the views and
rule documents it serves are written into its database by the Portal through `/q`, never mounted."""

import pytest


@pytest.fixture(scope="module")
def broker_pod(rendered):
    dev = rendered("dev")
    deployment = next(
        d for d in dev if d.get("kind") == "Deployment" and d["metadata"]["name"] == "context-broker"
    )
    return deployment["spec"]["template"]["spec"]


ADMIN_TOKEN = {"secretName": "context-broker-admin-token", "items": [{"key": "token", "path": "token"}]}


def test_the_broker_mounts_no_configuration_of_the_plane(broker_pod):
    """CC-01: one scratch directory and its own admin credential, nothing else; no repository,
    ConfigMap or other Secret volume."""
    volumes = broker_pod.get("volumes", [])
    scratch = [v for v in volumes if set(v) == {"name", "emptyDir"}]
    secrets = [v for v in volumes if "secret" in v]
    assert len(scratch) + len(secrets) == len(volumes), volumes
    assert [v["secret"] for v in secrets] == [ADMIN_TOKEN], secrets
    for container in broker_pod.get("initContainers", []) + broker_pod["containers"]:
        assert "git" not in container["name"], "no repository sidecar beside the broker"
        mounts = sorted(m["mountPath"] for m in container.get("volumeMounts", []))
        assert mounts in ([], ["/tmp"], ["/tmp", "/var/run/secrets/antares/admin"]), mounts
        assert not container.get("envFrom"), "the broker takes no bulk configuration of ours"


def test_the_broker_runs_the_surfaces_and_requires_its_admin_credential(broker_pod):
    """T-3642: the views, MCP, access, WebSocket, OGC and (T-3804) LinkML surfaces beside `/q`, the
    subject headers the gateway sets (platform T-3641), and `/q` behind the mounted secret with only
    the metrics open."""
    env = {e["name"]: e.get("value") for e in broker_pod["containers"][0].get("env", [])}
    assert env["ANTARES_API_SURFACES"].split(",") == ["admin", "views", "mcp", "access", "ws", "ws-views", "ogc", "linkml", "odata", "sta", "ical", "feed"]
    # T-3905: the view WebSocket is bounded for the one node, under the broker's own defaults.
    assert 0 < int(env["ANTARES_WS_MAX_CONNECTIONS_PER_TENANT"]) <= int(env["ANTARES_WS_MAX_CONNECTIONS"]) <= 1000
    assert 0 < int(env["ANTARES_WS_MAX_SUBSCRIPTIONS"]) <= 64
    assert 0 < int(env["ANTARES_WS_QUEUE"]) <= 256
    # T-4132: 24 h for an anonymous socket; the gateway cuts a token caller's at 300 s.
    assert env["ANTARES_WS_MAX_LIFETIME_SECS"] == "86400"
    assert "ANTARES_WS_ALLOWED_ORIGINS" not in env, "browsers reach the socket through the gateway alone"
    assert env["ANTARES_POLICY_SUBJECT_HEADERS"].split(",") == [
        "x-jc-subject", "x-jc-groups", "x-jc-roles", "x-jc-client", "x-jc-restricted", "x-jc-did",
        "x-jc-read-only", "x-jc-member"]
    assert not {"authorization", "cookie"} & set(env["ANTARES_POLICY_SUBJECT_HEADERS"].split(","))
    assert env["ANTARES_ADMIN_TOKEN_FILE"] == "/var/run/secrets/antares/admin/token"
    assert env["ANTARES_ADMIN_METRICS"] == "open"
    assert int(env["ANTARES_SCHEMA_MAX_BYTES"]) > 0
    # T-4414 (DM-97, owner 2026-10-18): 28 days of history where nothing more specific rules, and
    # never the partition-dropping horizon, which would cut a type its space keeps longer.
    assert env["ANTARES_RETENTION_HISTORY_DEFAULT"] == "P28D"
    assert "ANTARES_TEMPORAL_RETENTION_DAYS" not in env
    # T-3768: the joinedcontext engine decides every request, after the gateway's own hops carry
    # a subject (T-3767) and a participant's DID is a header it reads (T-3810).
    assert env["ANTARES_POLICY"] == "joinedcontext"


def test_the_broker_is_told_only_its_own_settings(broker_pod):
    """CC-01: its environment is the broker's own (`ANTARES_*`) and its database password; nothing
    names the configuration repository, a manifest or a gateway setting."""
    broker = broker_pod["containers"][0]
    names = {e["name"] for e in broker.get("env", [])}
    foreign = {n for n in names if not (n.startswith("ANTARES_") or n == "PGPASSWORD")}
    assert foreign == set()
    assert not any("REPO" in n or n.startswith("JC_") or n.startswith("GITSYNC") for n in names)


def test_the_broker_s_mcp_answers_the_host_the_gateway_calls(rendered, broker_pod):
    """T-3647: the gateway reaches an Endpoint's MCP on the broker by its Service name; the MCP
    surface refuses any Host it does not list (DNS rebinding), so the two must name the same."""
    env = {e["name"]: e.get("value") for e in broker_pod["containers"][0].get("env", [])}
    gateway = next(d for d in rendered("dev") if d.get("kind") == "Deployment"
                   and d["metadata"]["name"] == "context-gateway")
    gateway_env = {e["name"]: e.get("value") for c in gateway["spec"]["template"]["spec"]["containers"]
                   for e in c.get("env", [])}
    authority = gateway_env["JC_GATEWAY_BROKER_URL"].split("://", 1)[1].split("/", 1)[0]
    assert authority in env["ANTARES_MCP_ALLOWED_HOSTS"].split(","), (authority, env["ANTARES_MCP_ALLOWED_HOSTS"])


def test_the_broker_reaches_only_the_private_peers_it_names(rendered, broker_pod):
    """T-3909, Antares T-3908: private egress is off, and the list names exactly the broker's own
    Service (the endpoint of every federation registration), a Service host of this cluster: no
    wildcard, no address, no loopback. The gateway's R12 egress is gone (T-3925), so is its host."""
    env = {e["name"]: e.get("value") for e in broker_pod["containers"][0].get("env", [])}
    assert env["ANTARES_EGRESS_ALLOW_PRIVATE"] == "false"
    hosts = env["ANTARES_EGRESS_PRIVATE_HOSTS"].split(",")
    workloads = rendered("dev")
    env_of = lambda name: {e["name"]: e.get("value") for d in workloads
                           if d.get("kind") == "Deployment" and d["metadata"]["name"] == name
                           for c in d["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    host = lambda url: url.split("://", 1)[1].split("/", 1)[0].split(":", 1)[0]
    expected = {host(env_of("context-gateway")["JC_GATEWAY_BROKER_URL"])}
    assert sorted(hosts) == sorted(expected), (hosts, expected)
    for name in hosts:
        assert name.endswith(".svc.cluster.local") and "*" not in name, name
        assert not name.replace(".", "").isdigit() and "localhost" not in name, name


def test_the_broker_believes_provenance_from_the_gateway_alone(rendered, broker_pod):
    """T-5361 (ADR-N-054 2.3): the broker stores a write's channel, actor and request id only from
    the mesh identity of the gateway's own ServiceAccount; unset, every row said `api` and nobody."""
    env = {e["name"]: e.get("value") for e in broker_pod["containers"][0].get("env", [])}
    gateway = next(d for d in rendered("dev") if d.get("kind") == "Deployment"
                   and d["metadata"]["name"] == "context-gateway")
    account = gateway["spec"]["template"]["spec"]["serviceAccountName"]
    namespace = gateway["metadata"]["namespace"]
    assert env["ANTARES_PROVENANCE_FROM"] == (
        f"{account}.{namespace}.serviceaccount.identity.linkerd.cluster.local")
