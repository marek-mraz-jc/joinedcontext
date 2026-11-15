"""jc-assistant's crawl worker, judged as the deployment renders it (T-3052, Architecture/22 §1).

No environment lists the component until a platform image that carries the binary is pinned,
so the tests render dev with it added. What is coupled across files: the database and the role
the worker connects as, the forge token git-sync reads and the bootstrap that mints it into the
worker's namespace and restarts the worker, and the NetworkPolicies that let the worker reach
its database, the forge, DNS and public web addresses only."""

import pytest

# The `publicInternet` peer of components/networkpolicies (F-R2DEP2-023).
PRIVATE = {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8", "100.64.0.0/10", "0.0.0.0/8", "198.18.0.0/15", "224.0.0.0/4"}


def add_assistant(tree):
    path = tree / "deployment/environments/dev/global.yaml.gotmpl"
    text = path.read_text()
    if "\n  - assistant\n" in text:
        return  # dev lists the component itself since T-3181
    assert "\n  - functions\n" in text
    path.write_text(text.replace("\n  - functions\n", "\n  - functions\n  - assistant\n", 1))


RENDERED: list = []


@pytest.fixture
def docs(rendered_variant):
    # rendered_variant is function-scoped; the one render serves both tests.
    if not RENDERED:
        RENDERED.append(rendered_variant("dev", add_assistant))
    return RENDERED[0]


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


def test_the_worker_rendered_with_its_database_checkout_and_network(docs):
    pod = one(docs, "Deployment", "jc-assistant")["spec"]["template"]["spec"]
    worker = next(c for c in pod["containers"] if c["name"] != "git-sync" and c["name"] != "checkouts")
    assert worker["command"] == ["/usr/local/bin/jc-assistant"]
    assert "@sha256:" in worker["image"]
    env = env_of(worker)
    assert env["JC_ASSISTANT_DATABASE_URL"] == (
        "postgresql://assistant:$(PGPASSWORD)@postgres-cluster-rw.dev.svc.cluster.local:5432/assistant"
    )
    assert env["JC_ASSISTANT_REPO_DIR"] == "/repo/current"
    assert env["JC_ASSISTANT_ORG_DOMAIN"]
    assert env["JC_ASSISTANT_EMBED_THREADS"] == "1"
    password = next(e for e in worker["env"] if e["name"] == "PGPASSWORD")
    assert password["valueFrom"]["secretKeyRef"] == {"name": "db-assistant", "key": "password"}
    assert worker["securityContext"]["readOnlyRootFilesystem"] is True
    assert pod["automountServiceAccountToken"] is False
    assert worker["readinessProbe"]["httpGet"]["path"] == "/readyz"
    assert worker["resources"]["limits"]["memory"] == "1Gi"
    sidecars = {c["name"] for c in pod["containers"]} - {worker["name"]}
    assert "git-sync" in sidecars
    secrets = {v["secret"]["secretName"] for v in pod["volumes"] if "secret" in v}
    assert secrets <= {"gitea-token-gateway"}, "the read-only forge token and no client secret"
    # Its client is federated (PF-47, T-2868): the pod's projected token, never a secret.
    assert "JC_ASSISTANT_CLIENT_SECRET_FILE" not in env
    assert env["JC_ASSISTANT_CLIENT_ASSERTION_FILE"] == "/var/run/secrets/jc/keycloak/token"
    assert env["JC_ASSISTANT_PROXY_URL"] == "http://agent-proxy.dev.svc.cluster.local:8080"
    assert env["JC_ASSISTANT_GATEWAY_URL"] == "http://context-gateway.dev.svc.cluster.local:8080"
    assert env["JC_ASSISTANT_TOKEN_URL"].startswith("http://keycloak-app-keycloakx-http.")
    assert env["JC_ASSISTANT_LLM"]
    # A guide source's sections link into the Portal the people open (AG-118, T-3226).
    assert env["JC_ASSISTANT_PORTAL_URL"] == "https://portal.dev.joinedcontext.com"
    service = one(docs, "Service", "jc-assistant")
    assert [p["port"] for p in service["spec"]["ports"]] == [8080]

    # Its own database, owned by its own role, with the vector extension the role job creates.
    database = one(docs, "Database", "assistant")["spec"]
    assert database["owner"] == "assistant"
    assert {"ensure": "present", "name": "vector"} in database["extensions"]

    # The forge bootstrap may restart the worker when it mints the token again.
    restartable = {
        name
        for d in docs if d.get("kind") == "Role"
        for rule in d.get("rules", []) if "deployments" in rule.get("resources", [])
        for name in rule.get("resourceNames", [])
    }
    assert "jc-assistant" in restartable


def test_the_worker_reaches_its_database_the_forge_dns_and_public_addresses_only(docs):
    """And the chat's peers: the edge in; the agent proxy, the gateway and Keycloak out (T-3055)."""
    policy = one(docs, "NetworkPolicy", "assistant-worker")
    assert policy["spec"]["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "assistant-worker"}
    assert set(policy["spec"]["policyTypes"]) == {"Ingress", "Egress"}
    web = [r for r in policy["spec"]["egress"] if "ipBlock" in r["to"][0]]
    (rule,) = web
    block = rule["to"][0]["ipBlock"]
    assert block["cidr"] == "0.0.0.0/0"
    assert set(block["except"]) == PRIVATE
    assert sorted(p["port"] for p in rule["ports"]) == [80, 443]
    edge = [r for r in policy["spec"]["egress"] if "matchExpressions" in r["to"][0].get("podSelector", {})]
    (ingress,) = edge
    assert ingress["to"][0]["podSelector"]["matchExpressions"][0]["values"] == ["traefik", "ingress-nginx"]
    assert sorted(p["port"] for p in ingress["ports"]) == [443, 8443]
    peers = sorted(
        (tuple(sorted(peer["podSelector"]["matchLabels"].items())), tuple(p["port"] for p in r["ports"]))
        for r in policy["spec"]["egress"]
        for peer in r["to"]
        if "matchLabels" in peer.get("podSelector", {})
    )
    assert peers == sorted([
        ((("cnpg.io/cluster", "postgres-cluster"),), (5432,)),
        ((("app.kubernetes.io/instance", "gitea-forge"), ("app.kubernetes.io/name", "gitea")), (3000,)),
        ((("app.kubernetes.io/name", "agent-runner-proxy"),), (8080,)),
        ((("app.kubernetes.io/name", "context-gateway-gateway"),), (8080,)),
        ((("app.kubernetes.io/instance", "keycloak-app"),), (8080,)),
        ((("app.kubernetes.io/name", "functions-runtime"),), (8080,)),
        ((("k8s-app", "kube-dns"),), (53, 53)),
    ])
    (inbound,) = policy["spec"]["ingress"]
    assert [f["podSelector"]["matchLabels"] for f in inbound["from"]] == [
        {"app.kubernetes.io/name": "apisix"},
        {"app.kubernetes.io/name": "portal-portal"},
        {"app.kubernetes.io/name": "context-gateway-gateway"},
    ]
    assert [p["port"] for p in inbound["ports"]] == [8080]
    for name, port in (
        ("postgres-from-assistant", 5432),
        ("gitea-from-assistant", 3000),
        ("agent-proxy-from-assistant", 8080),
        ("gateway-from-assistant", 8080),
        ("keycloak-from-assistant", 8080),
        ("functions-from-assistant", 8080),
    ):
        (inbound,) = one(docs, "NetworkPolicy", name)["spec"]["ingress"]
        assert [f["podSelector"]["matchLabels"] for f in inbound["from"]] == [{"app.kubernetes.io/name": "assistant-worker"}]
        assert [p["port"] for p in inbound["ports"]] == [port]


def test_the_gateway_searches_the_assistants_documents_in_the_cluster(docs):
    """`search_documents` (EP-133, AG-119, T-4659): the gateway names the worker's Service, the hub
    names the cities' websites only, and the gateway's egress, plain and meshed, reaches the
    worker that admits it."""
    pod = one(docs, "Deployment", "context-gateway")["spec"]["template"]["spec"]
    env = env_of(pod["containers"][0])
    assert env["JC_GATEWAY_ASSISTANT_URL"] == "http://jc-assistant.dev.svc.cluster.local:8080"
    hub = env["JC_GATEWAY_HUB_KNOWLEDGE_SOURCES"].split(",")
    assert hub and all(entry.split("/")[1] == "web" for entry in hub), hub
    worker = {"app.kubernetes.io/name": "assistant-worker"}
    for name, port in (("context-gateway", 8080), ("context-gateway-linkerd-control-plane", 4143)):
        egress = one(docs, "NetworkPolicy", name)["spec"]["egress"]
        assert any(
            peer.get("podSelector", {}).get("matchLabels") == worker and [p["port"] for p in r["ports"]] == [port]
            for r in egress
            for peer in r["to"]
        ), name
    (meshed,) = one(docs, "NetworkPolicy", "assistant-allow-linkerd")["spec"]["ingress"]
    assert {"app.kubernetes.io/name": "context-gateway-gateway"} in [f["podSelector"]["matchLabels"] for f in meshed["from"]]


def test_the_chat_is_published_on_its_own_host_and_its_client_names_the_proxy(docs):
    """T-3055, AG-109: the edge route, and the Keycloak client whose token the proxy accepts."""
    import base64
    import json

    secret = one(docs, "Secret", "keycloak-config-keycloak-config-cli-config-realms")
    realm = json.loads(base64.b64decode(secret["data"]["dev.json"]))
    client = next(c for c in realm["clients"] if c["clientId"] == "jc-assistant")
    assert client["serviceAccountsEnabled"] and not client["standardFlowEnabled"] and not client["publicClient"]
    audiences = {m["config"]["included.custom.audience"] for m in client["protocolMappers"] if m["protocolMapper"] == "oidc-audience-mapper"}
    assert audiences == {"helsinki-agent-proxy", "jc-functions"}
    # AG-112: the functions runtime admits the assistant's client, and the assistant knows where it is.
    runtime = one(docs, "Deployment", "jc-functions")["spec"]["template"]["spec"]["containers"][0]
    runtime_env = {e["name"]: e.get("value") for e in runtime.get("env", [])}
    assert runtime_env["JC_FUNCTIONS_ASSISTANT_CALLER"] == "jc-assistant"
    worker = one(docs, "Deployment", "jc-assistant")["spec"]["template"]["spec"]["containers"]
    worker_env = {e["name"]: e.get("value") for c in worker for e in c.get("env", [])}
    assert worker_env["JC_ASSISTANT_FUNCTIONS_URL"] == "http://jc-functions.dev.svc.cluster.local:8080"
    # T-3057: the Portal asks the assistant's administration paths.
    portal = one(docs, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]
    portal_env = {e["name"]: e.get("value") for e in portal.get("env", [])}
    assert portal_env["JC_PORTAL_KNOWLEDGE_URL"] == "http://jc-assistant.dev.svc.cluster.local:8080"
    proxy = one(docs, "Deployment", "agent-proxy")["spec"]["template"]["spec"]["containers"][0]
    proxy_env = {e["name"]: e.get("value") for e in proxy.get("env", [])}
    assert proxy_env["JC_OIDC_CLIENT_ID"] == "helsinki-agent-proxy"
    config = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base")
    assert "assistant.dev.joinedcontext.com" in str(config["data"])
    assert "jc-assistant.dev.svc.cluster.local:8080" in str(config["data"])


def test_the_widget_is_served_from_the_assistants_host_which_the_chat_admits(docs):
    """AG-114: /d/* on assistant.{domain} reaches jc-assistant with no X-Frame-Options from the
    edge, and the service knows that host as its own origin, so its page may ask the chat."""
    import yaml

    worker = one(docs, "Deployment", "jc-assistant")["spec"]["template"]["spec"]["containers"]
    worker_env = {e["name"]: e.get("value") for c in worker for e in c.get("env", [])}
    assert worker_env["JC_ASSISTANT_PUBLIC_ORIGIN"] == "https://assistant.dev.joinedcontext.com"
    config = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base")
    edge = yaml.safe_load(next(v for v in config["data"].values() if "routes" in v and "#END" in v).replace("#END", ""))
    route = next(r for r in edge["routes"] if r.get("plugin_config_id") == "assistant-widget")
    assert route["uri"] == "/d/*" and route["host"] == "assistant.dev.joinedcontext.com"
    plugins = next(pc for pc in edge["plugin_configs"] if pc["id"] == "assistant-widget")["plugins"]
    assert "X-Frame-Options" not in plugins["response-rewrite"]["headers"]["set"]


def test_each_environment_runs_as_many_embedder_threads_as_its_cpu_limit():
    """T-4749: prod embeds its first full index on two cores; an environment that raises the CPU
    limit without the threads (or the other way round) wastes the core or starves the model."""
    import pathlib
    import re
    import yaml

    values = pathlib.Path(__file__).resolve().parents[1] / "components/assistant/values/worker"
    base = re.search(r'JC_ASSISTANT_EMBED_THREADS: "(\d+)"', (values / "base-values.yaml.gotmpl").read_text())
    for env in ("development", "production"):
        own = yaml.safe_load((values / f"{env}-values.yaml.gotmpl").read_text())
        threads = (own.get("env") or {}).get("JC_ASSISTANT_EMBED_THREADS", base.group(1))
        assert threads == str(own["resources"]["limits"]["cpu"]), env
