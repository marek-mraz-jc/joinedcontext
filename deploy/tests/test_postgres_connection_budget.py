"""T-4933: every client's pool ceiling fits the shared Postgres's max_connections.

On 2026-10-21 dev ran max_connections 50 with only the broker's pool sized against it. Keycloak's
pool (ceiling 100 by default) grew to 10 and Gitea's has no ceiling, the 47 slots left after
CNPG's superuser reserve filled, and Postgres refused Gitea's token lookups (53300) until every
Gitea token answered 401.

For each role the Cluster manages, this test reads the ceiling of the workload that logs in as
it from the render (or, where the ceiling is compiled into the program, from the constant
below with its source), multiplies by replicas, and asserts

    sum x 1.2 + superuser_reserved_connections <= max_connections

The 20 % headroom covers short-lived logins: a rolling update's second pod (pools open
lazily, so it starts near empty), init containers and Jobs that run a CLI, the CNPG operator's
own checks. A role this test does not know fails it: name its ceiling here before it ships.

T-5125: the ceilings also stay at or under ALERT_SHARE of max_connections, the threshold of the CNPG
chart's CNPGClusterHighConnectionsWarning, so that alert fires only for a client nobody counted.

T-5553: the same rule holds for `apps-db`, the Apps' database, and every CNPG Cluster's memory limit
holds shared_buffers plus max_connections backends at the measured memory of one backend:

    shared_buffers + max_connections x MIB_PER_BACKEND[cluster] <= limits.memory

max_connections, not the pools' sum, because Postgres lets that many backends in.
"""

import math
import re
import tomllib

import pytest
from test_postgres_memory_budget import PER_PROCESS_MIB

ENVIRONMENTS = ["local", "dev", "smartcity", "stage", "staging", "production", "minimal", "broker-only"]
# addons renders the opt-in components (jc, decidim) whose pools no live environment counts yet;
# its postgres-cluster renders without the PodMonitor the scrape test asks, so only the pool test
# reads it.
POOL_ENVIRONMENTS = [*ENVIRONMENTS, "addons", "jc-dev", "jc-prod"]
HEADROOM = 1.2
ALERT_SHARE = 0.8  # CNPGClusterHighConnectionsWarning, charts/cluster prometheus_rules (cnpg/cluster 0.7.0)
SUPERUSER_RESERVED_DEFAULT = 3
# Ceilings compiled into the programs (sqlx PgPoolOptions::max_connections):
# joinedcontext-portal src/db.rs and joinedcontext-platform crates/assistant/src/bin/jc-assistant.rs.
# joinedcontext-portal src/state.rs: the Portal's apps admin pool in apps-db, 4.
# jc (monorepo crates/portal/src/db.rs): the same Portal pool inside the one binary, 5; jc renders
# no apps-db, so the Portal opens no apps admin pool there.
COMPILED = {"portal": ("portal", 5), "assistant": ("jc-assistant", 10), "jc_apps_admin": ("portal", 4), "jc_portal": ("jc", 5)}
# Drivers' own defaults, used when the render sets nothing.
ANTARES_DEFAULT = 20  # antares-broker main.rs parse_pg_pool
KEYCLOAK_DEFAULT = 100  # Quarkus/Agroal KC_DB_POOL_MAX_SIZE
SQLALCHEMY_DEFAULT = (5, 10)  # QueuePool pool_size, max_overflow
CKAN_UWSGI_PROCESSES = 2  # ckan/ckan-base start_ckan.sh: `-p 2`, one thread each
WASM_DB_POOL_DEFAULT = 20  # joinedcontext-platform crates/wasm-host/src/main.rs JC_WASM_DB_POOL
SHARED_BUFFERS_DEFAULT_MIB = 128
# Memory of one backend, RSS of its own (Pss_Anon), at the work it serves.
# postgres-cluster: about 19 MiB per backend on dev (T-4933, test_postgres_memory_budget.py, which
# also sums its worker and maintenance memory).
# apps-db (T-5553, 2026-10-25): the cluster's image (cloudnative-pg/postgis 16-3.6-system-trixie)
# with 1000 App schemas, 40 backends each running one shard transaction (set_config role, a join,
# an insert) in K App schemas: 1.0 MiB at K=0, 3.4 at 100, 7.8 at 300, 22.3 at 1000 (10 backends:
# 22.1). A backend keeps the catalog of every App it served until sqlx retires it (30 min), so
# its memory grows with the Apps of its shard: 8 MiB holds 300 Apps per shard, three times the
# hundred dev and smartcity are sized for.
MIB_PER_BACKEND = {"postgres-cluster": PER_PROCESS_MIB, "apps-db": 8}


def workloads(docs: list[dict]) -> list[dict]:
    return [d for d in docs if d.get("kind") in ("Deployment", "StatefulSet")]


def named(docs: list[dict], name: str) -> list[dict]:
    return [d for d in workloads(docs) if d["metadata"]["name"] == name]


def env_of(workload: dict) -> dict[str, str]:
    return {
        e["name"]: e["value"]
        for c in workload["spec"]["template"]["spec"]["containers"]
        for e in c.get("env", [])
        if "value" in e
    }


def replicas(workload: dict) -> int:
    return int(workload["spec"].get("replicas", 1))


def antares(docs: list[dict], jc: bool = False) -> int:
    # jc runs the broker in its own process and logs in as jc_antares, counted apart.
    return sum(
        replicas(w) * int(env_of(w).get("ANTARES_PG_POOL", ANTARES_DEFAULT))
        for w in workloads(docs)
        if "ANTARES_DATABASE_URL" in env_of(w) and (w["metadata"]["name"] == "jc") == jc
    )


def jc_kaisatsu(docs: list[dict]) -> int:
    # kaisatsu in jc: its pool as kaisatsu_app (jc.toml [kaisatsu] db_max_connections, read by
    # the pool builder in kaisatsu store-pg, clamped to 50), counted under the database's owner
    # kaisatsu_migrate, plus the one login of the `kaisatsu migrate` init container.
    found = named(docs, "jc")
    assert found, "role kaisatsu_migrate has a database but no jc workload in the render"
    toml = [d["data"]["jc.toml"] for d in docs if d.get("kind") == "ConfigMap" and "jc.toml" in d.get("data", {})]
    assert len(toml) == 1, "the jc ConfigMap with jc.toml is not in the render"
    match = re.search(r"^db_max_connections = (\d+)$", toml[0], re.MULTILINE)
    assert match, "jc.toml sets no [kaisatsu] db_max_connections"
    return sum(replicas(w) for w in found) * (min(int(match[1]), 50) + 1)


def jc_module(module: str):
    # shako, daiya, dataroom and ekiben in jc: each pool as <module>_app (jc.toml [<module>.database]
    # max_connections, bins/jc/src/pg.rs), counted under the database's owner <module>_migrate,
    # plus that owner's one migration login at start.
    def count(docs: list[dict]) -> int:
        found = named(docs, "jc")
        assert found, f"role {module}_migrate has a database but no jc workload in the render"
        toml = [d["data"]["jc.toml"] for d in docs if d.get("kind") == "ConfigMap" and "jc.toml" in d.get("data", {})]
        assert len(toml) == 1, "the jc ConfigMap with jc.toml is not in the render"
        section = tomllib.loads(toml[0]).get(module, {}).get("database", {})
        assert "max_connections" in section, f"jc.toml sets no [{module}.database] max_connections"
        return sum(replicas(w) for w in found) * (section["max_connections"] + 1)

    return count


def jc_assistant(docs: list[dict]) -> int:
    # The assistant in jc migrates and serves as jc_assistant: [assistant.database] max_connections.
    found = named(docs, "jc")
    assert found, "role jc_assistant has a database but no jc workload in the render"
    (toml,) = [d["data"]["jc.toml"] for d in docs if d.get("kind") == "ConfigMap" and "jc.toml" in d.get("data", {})]
    section = tomllib.loads(toml).get("assistant", {}).get("database", {})
    assert "max_connections" in section, "jc.toml sets no [assistant.database] max_connections"
    return sum(replicas(w) for w in found) * section["max_connections"]


def keycloak(docs: list[dict]) -> int:
    return sum(
        replicas(w) * int(env_of(w).get("KC_DB_POOL_MAX_SIZE", KEYCLOAK_DEFAULT))
        for w in workloads(docs)
        if "KC_DB_URL_DATABASE" in env_of(w)
    )


def ckan(docs: list[dict]) -> int:
    # Each CKAN process holds a main engine and two DataStore engines (write, read).
    total = 0
    for w in workloads(docs):
        env = env_of(w)
        if "CKAN_SQLALCHEMY_URL" not in env:
            continue
        assert "UWSGI_OPTS" not in env, "count the uWSGI processes UWSGI_OPTS starts"
        main = int(env.get("CKAN___SQLALCHEMY__POOL_SIZE", SQLALCHEMY_DEFAULT[0])) + int(
            env.get("CKAN___SQLALCHEMY__MAX_OVERFLOW", SQLALCHEMY_DEFAULT[1])
        )
        datastore = int(env.get("CKAN__DATASTORE__SQLALCHEMY__POOL_SIZE", SQLALCHEMY_DEFAULT[0])) + int(
            env.get("CKAN__DATASTORE__SQLALCHEMY__MAX_OVERFLOW", SQLALCHEMY_DEFAULT[1])
        )
        total += replicas(w) * CKAN_UWSGI_PROCESSES * (main + 2 * datastore)
    return total


def gitea(docs: list[dict]) -> int:
    config = [d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config"]
    assert len(config) == 1, "the Gitea chart's inline config Secret is not in the render"
    match = re.search(r"^MAX_OPEN_CONNS=(\d+)$", config[0]["stringData"]["database"], re.MULTILINE)
    ceiling = int(match[1]) if match else 0
    assert ceiling > 0, "Gitea's [database] MAX_OPEN_CONNS is unset or 0: Gitea then opens connections without a ceiling"
    return sum(replicas(w) for w in named(docs, "gitea")) * ceiling


def decidim(docs: list[dict]) -> int:
    # Rails' pool is RAILS_MAX_THREADS per process (config/database.yml of the generated app).
    return sum(
        replicas(w) * int(env_of(w)["RAILS_MAX_THREADS"])
        for name in ("decidim-web", "decidim-sidekiq")
        for w in named(docs, name)
    )


def wasm_host(shard: str):
    # A shard's pool, and one App owner login while the Portal migrates an App of the shard.
    def count(docs: list[dict]) -> int:
        found = [w for w in workloads(docs) if env_of(w).get("JC_WASM_SHARD") == shard and "JC_WASM_DB_URL" in env_of(w)]
        assert found, f"role wasm_host_{shard} has no jc-wasm-host shard {shard} in the render"
        return sum(replicas(w) * int(env_of(w).get("JC_WASM_DB_POOL", WASM_DB_POOL_DEFAULT)) for w in found) + 1

    return count


def compiled(role: str):
    def count(docs: list[dict]) -> int:
        workload, ceiling = COMPILED[role]
        found = named(docs, workload)
        assert found, f"role {role} has a database but no {workload} workload in the render"
        return sum(replicas(w) for w in found) * ceiling

    return count


# A catalogue beside `catalogue` logs in as ckan_<name>; every CKAN is counted once, under `ckan`.
CLIENTS = {
    "antares": antares,
    "keycloak": keycloak,
    "gitea": gitea,
    "portal": compiled("portal"),
    "assistant": compiled("assistant"),
    "ckan": ckan,
    "jc_apps_admin": compiled("jc_apps_admin"),
    "decidim": decidim,
    "jc_portal": compiled("jc_portal"),
    "jc_antares": lambda docs: antares(docs, jc=True),
    "kaisatsu_migrate": jc_kaisatsu,
    **{f"{m}_migrate": jc_module(m) for m in ("shako", "daiya", "dataroom", "ekiben")},
    "jc_assistant": jc_assistant,
    # The branch previews' group never logs in; each member pv_{slug} has CONNECTION LIMIT 5, the
    # preview Portal's pool, and scripts/preview.sh starts at most PREVIEW_MAX = 2 of them (T-5548).
    "pv_previews": lambda docs: 2 * COMPILED["portal"][1],
}


def cnpg_clusters(docs: list[dict]) -> dict[str, dict]:
    return {d["metadata"]["name"]: d["spec"] for d in docs if d.get("kind") == "Cluster" and d["apiVersion"].startswith("postgresql.cnpg.io/")}


def counter(role: str):
    shard = re.fullmatch(r"wasm_host_(\d+)", role)
    return wasm_host(shard[1]) if shard else CLIENTS.get(role)


def budget(docs: list[dict], cluster: str = "postgres-cluster") -> tuple[dict[str, int], int, int]:
    spec = cnpg_clusters(docs)[cluster]
    parameters = spec["postgresql"].get("parameters", {})
    roles = [r["name"] for r in spec.get("managed", {}).get("roles", [])]
    clients = {"ckan" if r.startswith("ckan_") else r for r in roles}
    unknown = sorted(c for c in clients if counter(c) is None)
    assert not unknown, f"{cluster}: no pool ceiling known for role(s) {unknown}: add them to CLIENTS"
    pools = {client: counter(client)(docs) for client in clients}
    reserved = int(parameters.get("superuser_reserved_connections", SUPERUSER_RESERVED_DEFAULT))
    return pools, reserved, int(parameters.get("max_connections", 100))


def fits(pools: dict[str, int], reserved: int, max_connections: int) -> bool:
    total = sum(pools.values())
    return math.ceil(total * HEADROOM) + reserved <= max_connections and total <= ALERT_SHARE * max_connections


def mib(quantity: str, units: dict[str, int]) -> float:
    number, unit = re.fullmatch(r"(\d+)([A-Za-z]*)", quantity).groups()
    assert unit in units, f"unit of {quantity!r} not one of {sorted(units)}"
    return int(number) * units[unit]


KUBERNETES_UNITS = {"Mi": 1, "Gi": 1024, "Ki": 1 / 1024, "M": 1e6 / 2**20, "G": 1e9 / 2**20}
POSTGRES_UNITS = {"kB": 1 / 1024, "MB": 1, "GB": 1024, "": 8 / 1024}  # a bare number counts 8 kB pages


def memory_need(spec: dict, cluster: str) -> tuple[float, float]:
    """Return (shared_buffers + max_connections x MIB_PER_BACKEND, limits.memory), both in MiB."""
    assert cluster in MIB_PER_BACKEND, f"no measured memory per backend for Cluster {cluster}: measure it and add it to MIB_PER_BACKEND"
    parameters = spec["postgresql"].get("parameters", {})
    limit = spec.get("resources", {}).get("limits", {}).get("memory")
    assert limit, f"Cluster {cluster} sets no memory limit"
    buffers = mib(parameters["shared_buffers"], POSTGRES_UNITS) if "shared_buffers" in parameters else SHARED_BUFFERS_DEFAULT_MIB
    return buffers + int(parameters.get("max_connections", 100)) * MIB_PER_BACKEND[cluster], mib(limit, KUBERNETES_UNITS)


def test_the_memory_rule_refuses_apps_db_as_it_ran():
    # dev and smartcity before T-5553: max_connections 100, default shared_buffers, 768Mi.
    old = {"postgresql": {"parameters": {"max_connections": "100"}}, "resources": {"limits": {"memory": "768Mi"}}}
    need, limit = memory_need(old, "apps-db")
    assert (need, limit) == (928, 768)
    old["postgresql"]["parameters"] = {"max_connections": "60", "shared_buffers": "16384"}
    assert memory_need(old, "apps-db") == (608, 768)


def test_the_sum_counts_what_starved_gitea():
    # dev at 07:02Z on 2026-10-21 (deployment 13097d3, Keycloak not yet capped): the broker's
    # 16 and the counted pools fit 50, the ceilings nobody had set did not.
    t13097d3 = {"antares": 16, "keycloak": KEYCLOAK_DEFAULT, "ckan": 2 * sum(SQLALCHEMY_DEFAULT) * 3,
                "portal": 5, "assistant": 4, "gitea": 0}
    assert not fits(t13097d3, SUPERUSER_RESERVED_DEFAULT, 50)
    assert fits({"antares": 16, "keycloak": 7, "ckan": 8, "portal": 5, "assistant": 4, "gitea": 3}, 3, 55)
    # 45 of 55 passes the headroom (54 + 0) but holds the warning alert's 80 % (44) at every peak.
    assert not fits({"antares": 18, "keycloak": 7, "ckan": 8, "portal": 5, "assistant": 4, "gitea": 3}, 0, 55)


@pytest.mark.parametrize("env", POOL_ENVIRONMENTS)
def test_every_client_pool_fits_max_connections(rendered, env):
    docs = rendered(env)
    if not any(d.get("kind") == "Cluster" and d["metadata"]["name"] == "postgres-cluster" for d in docs):
        pytest.skip(f"{env} renders no postgres-cluster")
    pools, reserved, max_connections = budget(docs)
    need = math.ceil(sum(pools.values()) * HEADROOM) + reserved
    assert need <= max_connections, f"{env}: pools {pools} x {HEADROOM} + {reserved} reserved = {need} > max_connections {max_connections}"
    total = sum(pools.values())
    assert total <= ALERT_SHARE * max_connections, f"{env}: pools {pools} = {total} pass {ALERT_SHARE:.0%} of max_connections {max_connections}, where CNPGClusterHighConnectionsWarning fires"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_every_cnpg_cluster_is_scraped_and_alerts_on_connections_where_prometheus_watches(rendered, env):
    # cluster.monitoring.enabled follows global.metrics.enabled. Production 2026-10-22 set the key
    # the chart does not read (monitoring.enablePodMonitor) and rendered neither (T-5125).
    docs = rendered(env)
    clusters = [d["metadata"]["name"] for d in docs if d.get("kind") == "Cluster" and d["apiVersion"].startswith("postgresql.cnpg.io/")]
    if not clusters:
        pytest.skip(f"{env} renders no CNPG Cluster")
    watched = any(d.get("kind") == "ServiceMonitor" for d in docs)
    for name in clusters:
        monitors = [d for d in docs if d.get("kind") == "PodMonitor" and d["spec"]["selector"]["matchLabels"].get("cnpg.io/cluster") == name]
        rules = {
            r["alert"]: r["expr"]
            for d in docs if d.get("kind") == "PrometheusRule" and d["metadata"]["name"] == f"{name}-alert-rules"
            for g in d["spec"]["groups"] for r in g["rules"]
        }
        if not watched:
            # No Prometheus Operator CRDs where metrics are off: a PodMonitor would fail the apply.
            assert not monitors and not rules, f"{env}: {name} renders monitoring while global.metrics.enabled is false"
            continue
        assert [e["port"] for m in monitors for e in m["spec"]["podMetricsEndpoints"]] == ["metrics"], f"{env}: {name} has no PodMonitor"
        warning = rules.get("CNPGClusterHighConnectionsWarning", "")
        assert "max_connections" in warning and warning.rstrip().endswith(f"> {ALERT_SHARE * 100:.0f}"), f"{env}: {name} has no 80 % connection alert"


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_apps_db_pools_fit_max_connections(rendered, env):
    docs = rendered(env)
    if "apps-db" not in cnpg_clusters(docs):
        pytest.skip(f"{env} renders no apps-db")
    pools, reserved, max_connections = budget(docs, "apps-db")
    assert fits(pools, reserved, max_connections), (
        f"{env}: apps-db pools {pools} x {HEADROOM} + {reserved} reserved, or past {ALERT_SHARE:.0%}, do not fit max_connections {max_connections}"
    )


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_every_cnpg_cluster_memory_holds_max_connections(rendered, env):
    clusters = cnpg_clusters(rendered(env))
    if not clusters:
        pytest.skip(f"{env} renders no CNPG Cluster")
    for name, spec in clusters.items():
        need, limit = memory_need(spec, name)
        assert need <= limit, f"{env}: {name} shared_buffers + max_connections x {MIB_PER_BACKEND[name]} MiB = {need:.0f} MiB > memory limit {limit:.0f} MiB"
