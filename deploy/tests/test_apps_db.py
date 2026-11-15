"""apps-db, the database of the server WASM Apps, as the deployment renders it (T-3344, ADR-N-044,
AP-149).

Dev lists it beside jc-wasm-host since T-3361. What is coupled across files: the roles
CloudNativePG manages and the Secrets that hold their passwords, the `pg_hba` that admits those
logins alone over TLS, and the NetworkPolicy that lets only the Portal and the WASM host's shards
(and the operator and the cluster's own instances) reach it."""

import pytest


@pytest.fixture
def docs(rendered):
    return rendered("dev")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind} {name}: {len(found)}"
    return found[0]


def test_a_cluster_of_its_own_with_the_portals_login_and_one_per_shard(docs):
    cluster = one(docs, "Cluster", "apps-db")
    spec = cluster["spec"]
    assert one(docs, "Cluster", "postgres-cluster"), "apart from the platform's own cluster"
    assert "@sha256:" in spec["imageName"]
    assert spec["instances"] == 1
    initdb = spec["bootstrap"]["initdb"]
    assert (initdb["database"], initdb["owner"], initdb["secret"]["name"]) == ("apps", "jc_apps_admin", "db-apps-admin")
    roles = {r["name"]: r for r in spec["managed"]["roles"]}
    assert set(roles) == {"jc_apps_admin", "wasm_host_0", "wasm_host_1"}
    admin = roles["jc_apps_admin"]
    assert admin["createrole"] is True and admin["superuser"] is False and admin.get("createdb") is False
    for shard in (0, 1):
        host = roles[f"wasm_host_{shard}"]
        assert host["login"] is True and host["superuser"] is False
        # It inherits jc_set_config alone; the Portal grants App roles WITH INHERIT FALSE (T-3362).
        assert host["inRoles"] == ["jc_set_config"]
        assert host.get("createrole") is False
        assert host["passwordSecret"]["name"] == f"apps-host-{shard}-db"


def test_only_its_three_kinds_of_login_over_tls_and_everyone_else_refused(docs):
    hba = one(docs, "Cluster", "apps-db")["spec"]["postgresql"]["pg_hba"]
    assert hba[0] == "hostnossl all all all reject"
    assert hba[-1] == "host all all all reject", "nothing reaches CloudNativePG's appended catch-all"
    admitted = [line for line in hba if line.startswith("hostssl")]
    assert admitted == [
        "hostssl apps jc_apps_admin all scram-sha-256",
        "hostssl apps /^wasm_host_[0-9]+$ all scram-sha-256",
        "hostssl apps /^app_[0-9a-f]{16}_owner$ all scram-sha-256",
    ]
    assert not any(" app_" in line and "_owner" not in line for line in hba), "an App's run-time role never logs in"


def test_the_superuser_logs_in_over_tls_only_where_the_nightly_dump_runs(rendered):
    hba = one(rendered("smartcity"), "Cluster", "apps-db")["spec"]["postgresql"]["pg_hba"]
    assert "hostssl all postgres all scram-sha-256" in hba, "pg_dumpall of the backup part is refused"
    assert hba[-1] == "host all all all reject"


def test_every_password_is_a_generated_secret_where_its_reader_runs(docs):
    for name in ("db-apps-admin", "apps-host-0-db", "apps-host-1-db"):
        found = [d for d in docs if d.get("kind") in ("Secret", "ExternalSecret", "SealedSecret") and d["metadata"]["name"] == name]
        assert found, f"{name} is rendered"


def test_only_the_portal_the_shards_the_operator_and_its_own_instances_reach_it(docs):
    policy = next(
        d for d in docs
        if d.get("kind") == "NetworkPolicy" and d["spec"].get("podSelector", {}).get("matchLabels") == {"cnpg.io/cluster": "apps-db"}
        and "Egress" in d["spec"]["policyTypes"]
    )
    def name(peer):
        labels = peer.get("podSelector", {}).get("matchLabels", {})
        return labels.get("app.kubernetes.io/name") or labels.get("cnpg.io/cluster") or labels.get("cnpg.io/jobRole")

    rules = policy["spec"]["ingress"]
    # With metrics on (dev since T-5154) Prometheus scrapes the exporter port, nothing else.
    scrape = [rule for rule in rules if [name(peer) for peer in rule["from"]] == ["prometheus"]]
    assert all([p["port"] for p in rule["ports"]] == [9187] for rule in scrape), scrape
    names = {name(peer) for rule in rules if rule not in scrape for peer in rule["from"]}
    # apps-db-backup: the nightly dump and stat-statements of the `backup` part (F-DEP1-018).
    assert names == {"cloudnative-pg", "join", "apps-db", "portal-portal", "wasm-host-shard", "apps-db-backup"}, names


PORTAL_SUPERUSER_SQL = (
    "CREATE ROLE jc_set_config NOLOGIN",
    "REVOKE EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) FROM PUBLIC",
    "GRANT EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) TO jc_set_config",
)


def test_no_app_role_may_switch_again_or_run_handed_sql(docs):
    """T-3362: set_config only for jc_set_config, the query-running functions for nobody; the
    statements equal the Portal's apps_db/superuser.sql, which its same-shard test runs."""
    spec = one(docs, "Cluster", "apps-db")["spec"]
    post = spec["bootstrap"]["initdb"]["postInitApplicationSQL"]
    assert post[:3] == list(PORTAL_SUPERUSER_SQL)
    for function in ("query_to_xml(", "query_to_xmlschema(", "query_to_xml_and_xmlschema(", "cursor_to_xml(", "cursor_to_xmlschema(", "ts_stat(text)", "ts_stat(text, text)", "ts_rewrite("):
        assert any(line.startswith("REVOKE EXECUTE ON FUNCTION pg_catalog." + function) and line.endswith("FROM PUBLIC") for line in post), function
    roles = {r["name"]: r for r in spec["managed"]["roles"]}
    assert roles["jc_apps_admin"]["inRoles"] == ["jc_set_config"]


def test_the_operator_reaches_every_clusters_status_port(docs):
    """CNPG polls each instance's manager on 8000; an operator whose egress names only the
    platform's cluster left apps-db "not ready" on dev (HTTP communication issue, T-3361)."""
    operator = {"app.kubernetes.io/name": "cloudnative-pg"}
    reached = {
        (peer["podSelector"]["matchLabels"].get("cnpg.io/cluster"), port["port"])
        for d in docs
        if d.get("kind") == "NetworkPolicy" and d["spec"]["podSelector"].get("matchLabels") == operator
        for rule in d["spec"].get("egress", [])
        for peer in rule.get("to", [])
        if "podSelector" in peer
        for port in rule.get("ports", [])
    }
    for cluster in [d["metadata"]["name"] for d in docs if d.get("kind") == "Cluster"]:
        assert (cluster, 8000) in reached, f"no operator egress to {cluster}'s instance manager on 8000"
        assert (cluster, 4143) in reached, f"no operator egress to {cluster}'s inbound proxy on 4143"
