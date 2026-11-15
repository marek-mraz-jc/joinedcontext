"""T-0020: point-in-time recovery of the CNPG cluster (OPS-10, OPS-11).

A restore is a values change, not a second component: `postgres.cluster.recovery.enabled`
bootstraps the same cluster, under the same name every consumer connects to, from the WAL
archive at a chosen instant instead of from `initdb`. The `recovery-test` environment is that
values change, rendered on every push so the procedure in Runbook 6 is never written for the
first time during an incident.
"""

import datetime
import shutil
import subprocess

import pytest
import yaml

# Writes `deployment/environments/testing` in its own copy of the tree (`own_tree`), so it
# shares nothing with another module. Its own xdist group keeps the module on one worker, so its
# module-scoped renders happen once rather than once per worker (T-2895).
pytestmark = pytest.mark.xdist_group("postgres-recovery")

SOURCE_ARCHIVE = "s3://jc-backups/postgres"
TARGET_TIME = datetime.datetime(2026, 8, 15, 14, 30, tzinfo=datetime.timezone.utc)


@pytest.fixture(scope="module")
def restore(rendered):
    return rendered("recovery-test")


@pytest.fixture(scope="module")
def production(rendered):
    return rendered("production")


def cluster(docs: list[dict]) -> dict:
    found = [d for d in docs if d.get("kind") == "Cluster"]
    assert len(found) == 1, f"expected exactly one Cluster, got {len(found)}"
    return found[0]


def test_the_restore_bootstraps_from_the_archive_at_the_target_time(restore):
    """`initdb` here would be the worst outcome of the two: an empty database that starts
    cleanly, passes every probe and has lost everything the restore was for."""
    bootstrap = cluster(restore)["spec"]["bootstrap"]
    assert "initdb" not in bootstrap
    assert bootstrap["recovery"]["source"] == "objectStoreRecoveryCluster"
    # Helm writes the instant unquoted, so YAML resolves it: reading it back as the instant
    # asked for is the ISO 8601 round-trip the restore depends on.
    target = bootstrap["recovery"]["recoveryTarget"]["targetTime"]
    if isinstance(target, str):
        target = datetime.datetime.fromisoformat(target.replace("Z", "+00:00"))
    assert target == TARGET_TIME


def test_the_restore_reads_the_source_cluster_archive(restore):
    """`serverName` is the directory the source cluster wrote under. Point it at the wrong
    name and the recovery finds no WAL at all."""
    external = cluster(restore)["spec"]["externalClusters"]
    assert [e["name"] for e in external] == ["objectStoreRecoveryCluster"]
    store = external[0]["barmanObjectStore"]
    assert store["serverName"] == "postgres-cluster"
    assert store["destinationPath"] == SOURCE_ARCHIVE
    assert store["endpointURL"]


def test_the_restore_does_not_archive_over_the_backup_it_restores_from(restore):
    """The restored cluster carries the source cluster's name, so barman would write its
    WAL under the same key prefix it is reading — destroying the only copy of the history
    the restore may still need. The archive it writes to must be a different one."""
    spec = cluster(restore)["spec"]
    written = spec["backup"]["barmanObjectStore"]["destinationPath"]
    read = spec["externalClusters"][0]["barmanObjectStore"]["destinationPath"]
    assert written != read
    assert spec["backup"]["retentionPolicy"] == "30d"


def test_the_archive_credentials_are_a_reference_only(restore):
    """Read credentials for the source archive are operator-supplied like the backup ones:
    the chart must name an existing Secret, never render one from values."""
    store = cluster(restore)["spec"]["externalClusters"][0]["barmanObjectStore"]
    for field in ("accessKeyId", "secretAccessKey"):
        assert store["s3Credentials"][field]["name"] == "postgres-backup-s3"
    names = [d["metadata"]["name"] for d in restore if d.get("kind") == "Secret"]
    assert not [n for n in names if "s3-creds" in n or n == "postgres-backup-s3"]


def test_a_normal_environment_still_bootstraps_with_initdb(production):
    """The recovery block is inert unless an operator turns it on. A running database
    re-bootstrapped by a stray default is the failure this guards."""
    spec = cluster(production)["spec"]
    assert "initdb" in spec["bootstrap"] and "recovery" not in spec["bootstrap"]
    assert "externalClusters" not in spec


@pytest.mark.parametrize(
    "overrides, message",
    [
        ({"destinationPath": SOURCE_ARCHIVE}, "archive its own WAL over the backup"),
        ({"targetTime": "last tuesday"}, "must be an ISO 8601 instant"),
    ],
    ids=["archives-over-its-source", "target-time-is-not-a-timestamp"],
)
def test_a_restore_that_would_lose_data_or_miss_its_target_stops_the_render(own_tree, overrides, message):
    """Both mistakes are silent at apply time — the first overwrites the history, the second
    recovers to a point nobody chose — so they are refused while they are still text."""
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    backups = {
        "enabled": True,
        "endpointURL": "https://s3.example.org",
        "destinationPath": overrides.get("destinationPath", "s3://jc-backups/postgres-restored"),
        "region": "eu-central-1",
        "bucket": "jc-backups",
    }
    recovery = {"enabled": True, "targetTime": overrides.get("targetTime", "2026-08-15T14:30:00Z"),
                "destinationPath": SOURCE_ARCHIVE}
    env_dir = own_tree / "deployment/environments/testing"
    shutil.rmtree(env_dir, ignore_errors=True)
    env_dir.mkdir(parents=True)
    (env_dir / "global.yaml.gotmpl").write_text(
        yaml.safe_dump({"postgres": {"cluster": {"backups": backups, "recovery": recovery}}})
    )
    try:
        result = subprocess.run(
            ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "testing", "template",
             "--skip-deps", "-q", "--selector", "component=postgres"],
            cwd=str(own_tree), capture_output=True, text=True,
        )
    finally:
        shutil.rmtree(env_dir, ignore_errors=True)
    assert result.returncode != 0, result.stdout
    assert message in result.stderr, result.stderr
