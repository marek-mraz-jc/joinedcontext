"""Every ContextBroker beside `main` is its own island (T-3989, ADR-N-046 §2.5, PF-110, PF-114).

Dev rendered with two further brokers in `global.contextBrokers`: each is a release of its own
with its own CloudNativePG cluster, login, admin token, Keycloak client and audience, and
NetworkPolicies that let no broker reach another's database; `main` renders exactly as before.
The gateway and the Portal find a further broker by the URL template.
"""

import base64
import copy
import json
import shutil
import subprocess

import pytest
import yaml

from conftest import PROJECT_ROOT, TREE_IGNORE, load_yaml_all, set_global

# archive takes every default: size small, the environment's storage class.
BROKERS = [{"name": "archive"},
           {"name": "sensors", "size": "medium", "storageClass": "hcloud-volumes"}]
OTHER = {"archive": "sensors", "sensors": "archive"}


def render(tmp_path_factory, brokers, env="dev") -> subprocess.CompletedProcess:
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    if not (PROJECT_ROOT / f"deployment/environments/{env}").is_dir():
        pytest.skip("run `just _dev-assemble` first")
    tree = tmp_path_factory.mktemp("brokers") / "repo"
    shutil.copytree(PROJECT_ROOT, tree, ignore=TREE_IGNORE)
    set_global(tree, "contextBrokers", brokers)
    out = tree / f"{env}.yaml"
    result = subprocess.run([str(tree / "scripts/render.sh"), env, str(out)], cwd=str(tree),
                            capture_output=True, text=True)
    result.out = out
    return result


@pytest.fixture(scope="module")
def rendered_file(tmp_path_factory):
    result = render(tmp_path_factory, BROKERS)
    assert result.returncode == 0, result.stderr
    return result.out


@pytest.fixture(scope="module")
def docs(rendered_file):
    return [d for d in load_yaml_all(rendered_file.read_text()) if isinstance(d, dict)]


def named(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind} {name}: {len(found)}"
    return found[0]


def env_of(deployment) -> dict:
    return {e["name"]: e for e in deployment["spec"]["template"]["spec"]["containers"][0].get("env", [])}


def test_each_broker_is_a_release_of_its_own_and_main_keeps_its_names(docs):
    main = named(docs, "Deployment", "context-broker")
    assert main["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"] == "context-broker-broker"
    assert named(docs, "Service", "context-broker")
    for name in OTHER:
        broker = named(docs, "Deployment", f"context-broker-{name}")
        assert broker["spec"]["replicas"] == 1
        pod = broker["spec"]["template"]
        assert pod["metadata"]["labels"]["app.kubernetes.io/name"] == f"context-broker-{name}"
        assert pod["spec"]["serviceAccountName"] == f"context-broker-{name}"
        assert named(docs, "Service", f"context-broker-{name}")["spec"]["ports"][0]["port"] == 8080
        env = env_of(broker)
        assert env["PGPASSWORD"]["valueFrom"]["secretKeyRef"]["name"] == f"db-antares-{name}"
        assert f"@context-broker-{name}-db-rw." in env["ANTARES_DATABASE_URL"]["value"]
        assert env["ANTARES_MCP_ALLOWED_HOSTS"]["value"].startswith(f"context-broker-{name}.")
        tokens = {v["secret"]["secretName"] for v in pod["spec"]["volumes"] if "secret" in v}
        assert tokens == {f"context-broker-{name}-admin-token"}
        assert "@sha256:" in pod["spec"]["containers"][0]["image"]
    # small is today's development broker, medium today's production one.
    assert named(docs, "Deployment", "context-broker-archive")["spec"]["template"]["spec"]["containers"][0]["resources"] == \
        main["spec"]["template"]["spec"]["containers"][0]["resources"]
    assert named(docs, "Deployment", "context-broker-sensors")["spec"]["template"]["spec"]["containers"][0]["resources"]["limits"] == \
        {"cpu": "2", "memory": "1Gi"}


def test_each_broker_owns_a_cluster_with_its_own_login_over_tls(docs):
    # An empty class takes the environment's storage.storageClass.rwo, local-path on dev.
    for name, storage in (("archive", ("2Gi", "local-path")), ("sensors", ("20Gi", "hcloud-volumes"))):
        spec = named(docs, "Cluster", f"context-broker-{name}-db")["spec"]
        assert "@sha256:" in spec["imageName"]
        initdb = spec["bootstrap"]["initdb"]
        assert (initdb["database"], initdb["owner"], initdb["secret"]["name"]) == ("antares", "antares", f"db-antares-{name}")
        assert {"CREATE EXTENSION IF NOT EXISTS postgis", "CREATE EXTENSION IF NOT EXISTS btree_gin"} <= set(initdb["postInitApplicationSQL"])
        assert spec["postgresql"]["pg_hba"] == ["hostnossl all all all reject", "hostssl antares antares all scram-sha-256",
                                               "host all all all reject"]
        assert (spec["storage"]["size"], spec["storage"].get("storageClass", "")) == storage
    secrets = {d["metadata"]["name"] for d in docs if d.get("kind") == "Secret"}
    for name in OTHER:
        assert {f"db-antares-{name}", f"context-broker-{name}-admin-token", f"context-broker-{name}-secret-key"} <= secrets
        ref = env_of(named(docs, "Deployment", f"context-broker-{name}"))["ANTARES_SECRET_KEY"]["valueFrom"]["secretKeyRef"]
        assert ref == {"name": f"context-broker-{name}-secret-key", "key": "key"}, "each broker seals with its own key"


def test_no_broker_reaches_another_brokers_database(docs):
    policies = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "NetworkPolicy"}
    for name, other in OTHER.items():
        db = policies[f"broker-{name}-db"]["spec"]
        assert db["podSelector"]["matchLabels"] == {"cnpg.io/cluster": f"context-broker-{name}-db"}
        admitted = {p["podSelector"]["matchLabels"].get("app.kubernetes.io/name")
                    for rule in db["ingress"] for p in rule["from"]}
        assert f"context-broker-{name}" in admitted
        assert not {a for a in admitted if a and a.startswith("context-broker-")} - {f"context-broker-{name}"}
        pod = policies[f"broker-{name}"]["spec"]
        reached = {p["podSelector"]["matchLabels"].get("cnpg.io/cluster") for rule in pod["egress"] for p in rule.get("to", [])
                   if "podSelector" in p}
        assert reached - {None} == {f"context-broker-{name}-db"}
        callers = {p["podSelector"]["matchLabels"]["app.kubernetes.io/name"] for rule in pod["ingress"] for p in rule["from"]}
        # Prometheus too, where monitoring.prometheusNamespace names its namespace (not on dev).
        assert {"context-gateway-gateway", "portal-portal"} <= callers <= {"context-gateway-gateway", "portal-portal", "prometheus"}
        assert f"context-broker-{other}" not in json.dumps(db) + json.dumps(pod)
        # F-R2DEP2-017: a further broker delivers a notification straight to its subscriber, as
        # `main` does, on the same ports to the same public range; nowhere inside the cluster.
        def delivery(spec):
            return [(rule["to"], rule["ports"]) for rule in spec["egress"]
                    if any("ipBlock" in p for p in rule.get("to", []))]
        assert delivery(pod) == delivery(policies["context-broker"]["spec"]), delivery(pod)
    # Neither reaches the platform's cluster, which only `main` does.
    platform = [d for d in policies.values() if d["spec"]["podSelector"].get("matchLabels", {}).get("cnpg.io/cluster") == "postgres-cluster"]
    assert "context-broker-archive" not in json.dumps(platform) and "context-broker-sensors" not in json.dumps(platform)


def test_the_policy_gate_refuses_a_database_open_to_another_broker(rendered_file, docs, tmp_path):
    binary = shutil.which("conftest")
    if not binary:
        pytest.skip("conftest is not installed")
    gate = lambda path: subprocess.run([binary, "test", "--all-namespaces", "-p", str(PROJECT_ROOT / "policies"), str(path)],
                                       capture_output=True, text=True)
    assert gate(rendered_file).returncode == 0
    db = copy.deepcopy(named(docs, "NetworkPolicy", "broker-archive-db"))
    db["spec"]["ingress"][-1]["from"].append({"podSelector": {"matchLabels": {"app.kubernetes.io/name": "context-broker-sensors"}}})
    platform = copy.deepcopy(db)
    platform["metadata"]["name"] = "platform-db"
    platform["spec"]["podSelector"] = {"matchLabels": {"cnpg.io/cluster": "postgres-cluster"}}
    pod = copy.deepcopy(named(docs, "NetworkPolicy", "broker-archive"))
    pod["spec"]["egress"].append({"to": [{"podSelector": {"matchLabels": {"cnpg.io/cluster": "postgres-cluster"}}}]})
    open_ns = copy.deepcopy(named(docs, "NetworkPolicy", "broker-sensors-db"))
    open_ns["spec"]["ingress"].append({"from": [{"namespaceSelector": {}}]})
    bad = tmp_path / "bad.yaml"
    bad.write_text(yaml.safe_dump_all([db, platform, pod, open_ns]))
    result = gate(bad)
    assert result.returncode == 1
    for expected in ("the database of context-broker-archive admits the broker context-broker-sensors",
                     "the database of context-broker-broker admits the broker context-broker-sensors",
                     "the broker context-broker-archive reaches the database of context-broker-broker",
                     "the database of context-broker-sensors admits a peer that names no pod"):
        assert expected in result.stdout, result.stdout


def test_the_gateway_and_the_portal_find_a_further_broker_by_template_and_main_by_its_old_url(docs):
    for deployment, var in (("context-gateway", "JC_GATEWAY_BROKER"), ("portal", "JC_PORTAL_BROKER")):
        env = env_of(named(docs, "Deployment", deployment))
        assert env[f"{var}_URL"]["value"] == "http://context-broker.dev.svc.cluster.local:8080"
        assert env[f"{var}_URL_TEMPLATE"]["value"] == "http://context-broker-{broker}.dev.svc.cluster.local:8080"


def test_each_broker_has_a_federated_client_bound_to_its_own_audience(docs):
    realm = next(d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"].endswith("config-cli-config-realms"))
    clients = {c["clientId"]: c for c in json.loads(base64.b64decode(realm["data"]["dev.json"]))["clients"]}
    for name in OTHER:
        client = clients[f"context-broker-{name}"]
        assert client["clientAuthenticatorType"] == "federated-jwt" and "secret" not in client
        assert client["attributes"]["jwt.credential.sub"] == f"system:serviceaccount:dev:context-broker-{name}"
        audiences = {m["config"].get("included.custom.audience") for m in client["protocolMappers"]} - {None}
        assert audiences == {f"context-broker-{name}"}


def test_prometheus_scrapes_each_broker_where_monitoring_is_deployed(tmp_path_factory):
    """Dev deploys no monitoring; production does, with one ServiceMonitor per broker."""
    result = render(tmp_path_factory, BROKERS, "production")
    assert result.returncode == 0, result.stderr
    monitors = {d["metadata"]["name"]: d["spec"] for d in load_yaml_all(result.out.read_text())
                if isinstance(d, dict) and d.get("kind") == "ServiceMonitor"}
    for name in ("broker", *OTHER):
        monitor = monitors["context-broker" if name == "broker" else f"context-broker-{name}"]
        assert monitor["selector"]["matchLabels"] == {"app.kubernetes.io/name": f"context-broker-{name}"}
        assert [(e["port"], e["path"]) for e in monitor["endpoints"]] == [("metrics", "/q/metrics")]


@pytest.mark.parametrize("brokers, reason", [
    ([{"name": "main"}], "is not a broker name"),
    ([{"name": "broker"}], "is not a broker name"),
    ([{"name": "a"}, {"name": "a"}], "is not a broker name"),
    ([{"name": "Archive_1"}], "is not a broker name"),
    ([{"name": "a-very-long-broker-name"}], "is not a broker name"),
    ([{"name": "archive", "size": "huge"}], "is none of small, medium, large"),
])
def test_a_bad_broker_entry_aborts_the_render(tmp_path_factory, brokers, reason):
    result = render(tmp_path_factory, brokers)
    assert result.returncode != 0 and reason in result.stderr, result.stderr[-2000:]
