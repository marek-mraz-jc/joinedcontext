"""T-0019: the CNPG cluster archives WAL to object storage and speaks TLS only.

T-0262 adds the image it runs: no Kyverno policy judges a Cluster resource, so the
digest of the database image is only ever checked here."""

import pytest


@pytest.fixture(scope="module")
def production(rendered):
    return rendered("production")


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


def only(docs: list[dict], kind: str) -> dict:
    found = [d for d in docs if d.get("kind") == kind]
    assert len(found) == 1, f"expected exactly one {kind}, got {len(found)}"
    return found[0]


def test_cluster_requires_tls(production, local):
    """CNPG appends its own `host all all all scram-sha-256`, so the reject rule — not the
    hostssl rule — is what stops a client that refuses TLS (BSI TR-03187 CT-9)."""
    for docs in (production, local):
        pg_hba = only(docs, "Cluster")["spec"]["postgresql"]["pg_hba"]
        assert pg_hba == [
            "hostnossl all all all reject",
            "hostssl all all all scram-sha-256",
        ]


def test_production_archives_wal_to_object_storage(production):
    backup = only(production, "Cluster")["spec"]["backup"]
    store = backup["barmanObjectStore"]
    assert store["destinationPath"].startswith("s3://")
    assert store["endpointURL"]
    assert backup["retentionPolicy"] == "30d"
    assert store["wal"]["compression"] == "gzip" and store["wal"]["encryption"] == "AES256"
    assert store["data"]["compression"] == "gzip" and store["data"]["encryption"] == "AES256"


def test_object_storage_credentials_are_a_reference_only(production):
    """No access key may reach a manifest: barman gets a Secret name, and the chart
    must not have created that Secret from values it could only have read in plaintext."""
    store = only(production, "Cluster")["spec"]["backup"]["barmanObjectStore"]
    creds = store["s3Credentials"]
    assert creds["accessKeyId"]["name"] == "postgres-backup-s3"
    assert creds["accessKeyId"]["key"] == "ACCESS_KEY_ID"
    assert creds["secretAccessKey"]["name"] == "postgres-backup-s3"
    assert creds["secretAccessKey"]["key"] == "ACCESS_SECRET_KEY"
    rendered_secrets = [d["metadata"]["name"] for d in production if d.get("kind") == "Secret"]
    assert not [n for n in rendered_secrets if "s3-creds" in n or n == "postgres-backup-s3"]


def test_production_has_a_daily_scheduled_backup(production):
    schedule = only(production, "ScheduledBackup")["spec"]
    assert schedule["method"] == "barmanObjectStore"
    # CNPG cron carries a leading seconds field, so a five-field expression is silently wrong.
    assert len(schedule["schedule"].split()) == 6, schedule["schedule"]


def test_backups_stay_off_without_an_object_store(local):
    """A half-configured backup is worse than an honestly absent one."""
    assert only(local, "Cluster")["spec"].get("backup") is None
    assert not [d for d in local if d.get("kind") == "ScheduledBackup"]


def test_the_database_image_is_pinned_by_digest(production, local):
    """The Cluster is a custom resource, not a workload: the pods it produces are built by
    the operator, long after any admission policy has looked at this manifest. A tag here
    would be the one image reference nothing else can catch (BSI TR-03187 AR-4)."""
    for docs in (production, local):
        image = only(docs, "Cluster")["spec"]["imageName"]
        assert "@sha256:" in image, image


@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_managed_role_names_are_unique(rendered, env):
    """CNPG rejects the whole Cluster when a name repeats in `spec.managed.roles`:

        admission webhook "vcluster.cnpg.io" denied the request:
        spec.managed.roles: Invalid value: "ckan": Role name is duplicate of another

    The roles are generated one per database, but a user may own SEVERAL — CKAN keeps its
    catalogue and its DataStore apart and owns both (EP-65) — so two databases with one user
    used to emit that user twice. Nothing caught it: the render is valid YAML and no Kyverno
    policy judges a Cluster, so the first thing to object was the live webhook, at `dev-apply`.
    """
    # Every Cluster: dev runs the Apps' own beside the platform's (apps-db, T-3361).
    for cluster in [d for d in rendered(env) if d.get("kind") == "Cluster"]:
        names = [role["name"] for role in cluster["spec"].get("managed", {}).get("roles", [])]
        duplicates = sorted({name for name in names if names.count(name) > 1})
        assert not duplicates, (
            f"{env} renders {duplicates} more than once in {cluster['metadata']['name']}'s "
            "spec.managed.roles; CNPG refuses the Cluster. One role per user, not per database."
        )


@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_every_embedded_database_has_a_role_to_own_it(rendered, env):
    """Deduplicating by user must not drop a user: every embedded database's owner still needs
    a role, or the database is created and nothing can log into it."""
    docs = rendered(env)
    # A database's owner needs its role in the Cluster the database lives in.
    roles = {
        d["metadata"]["name"]: {role["name"] for role in d["spec"].get("managed", {}).get("roles", [])}
        for d in docs
        if d.get("kind") == "Cluster"
    }
    missing = sorted(
        f"{d['spec']['cluster']['name']}/{d['spec']['owner']}"
        for d in docs
        if d.get("kind") == "Database"
        and d.get("spec", {}).get("owner")
        and d["spec"]["owner"] not in roles.get(d["spec"]["cluster"]["name"], set())
    )
    assert not missing, f"{env} declares databases owned by {missing} with no managed role"


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def test_every_dev_cluster_that_holds_data_has_a_daily_backup(dev):
    """T-5662: apps-db on dev had no backup at all while postgres-cluster archived nightly."""
    clusters = {d["metadata"]["name"]: d for d in dev if d.get("kind") == "Cluster"}
    assert {"postgres-cluster", "apps-db"} <= set(clusters)
    scheduled = {d["spec"]["cluster"]["name"] for d in dev if d.get("kind") == "ScheduledBackup"}
    assert scheduled == set(clusters)
    for name, cluster in clusters.items():
        store = cluster["spec"]["backup"]["barmanObjectStore"]
        assert store["endpointURL"].startswith("http://artifact-store."), name
        # RustFS has no KMS and refuses SSE-S3 (T-5289).
        assert "encryption" not in store["wal"] or not store["wal"]["encryption"], name


def test_apps_db_archives_with_a_key_that_reaches_its_own_prefix_alone(dev):
    """apps-db serves tenant Apps: it must not hold the key that can delete the platform's backups."""
    apps = next(d for d in dev if d.get("kind") == "Cluster" and d["metadata"]["name"] == "apps-db")
    creds = apps["spec"]["backup"]["barmanObjectStore"]["s3Credentials"]
    assert creds["accessKeyId"]["name"] == creds["secretAccessKey"]["name"] == "apps-db-backup-artifact-store"
    job = next(d for d in dev if d.get("kind") == "Job" and d["metadata"]["name"] == "artifact-store-bucket")
    container = job["spec"]["template"]["spec"]["containers"][0]
    script = "\n".join(container.get("command", []) + container.get("args", []))
    assert '"$backups" "$backups" "apps-db" > /tmp/policy.json' in script
    assert "arn:aws:s3:::%s/%s/*" in script
    readers = {e["valueFrom"]["secretKeyRef"]["name"] for e in container["env"] if "valueFrom" in e}
    assert "apps-db-backup-artifact-store" in readers


def test_apps_db_and_the_store_let_the_archive_through_on_dev(dev):
    policies = {d["metadata"]["name"]: d["spec"] for d in dev if d.get("kind") == "NetworkPolicy"}
    store_peers = [p["podSelector"]["matchLabels"] for p in policies["artifact-store"]["ingress"][0]["from"]]
    assert {"cnpg.io/cluster": "apps-db"} in store_peers
    egress = [r for r in policies["apps-db"]["egress"]
              if any(t.get("podSelector", {}).get("matchLabels") == {"app.kubernetes.io/component": "store"} for t in r["to"])]
    assert [p["port"] for r in egress for p in r["ports"]] == [9000]


def test_dev_keeps_one_day_of_backups_and_production_keeps_thirty(rendered, production):
    """T-5772, T-6144: dev's WAL archive (about 24 G a day) and its 13 G bases share the node disk
    with Postgres; 2d held three bases and filled the disk to 88 %, so dev keeps one day. The
    override is dev's alone: production keeps 30 days."""
    dev = [d for d in rendered("dev") if d.get("kind") == "Cluster" and d["metadata"]["name"] == "postgres-cluster"]
    assert len(dev) == 1
    assert dev[0]["spec"]["backup"]["retentionPolicy"] == "1d"
    assert only(production, "Cluster")["spec"]["backup"]["retentionPolicy"] == "30d"
