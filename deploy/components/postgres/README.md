# Postgres

Central managed PostgreSQL database for the joinedcontext platform, run by the
CloudNativePG operator.

Deploy the component with:

```bash
helmfile apply -i --selector component=postgres -f helmfile.yaml.gotmpl
```

## Transport encryption

CloudNativePG issues the server certificate itself, so `SHOW ssl` is `on` with no extra
configuration. That alone does not make TLS mandatory: the operator appends
`host all all all scram-sha-256` as its own last `pg_hba` rule, and a client that refuses
TLS falls straight through to it. So `pg_hba` opens with

```
hostnossl all all all reject
hostssl   all all all scram-sha-256
```

An unencrypted connection is refused before it can reach the appended catch-all
(BSI TR-03187 CT-9). Clients need nothing beyond libpq's default `sslmode=prefer`, which
negotiates TLS whenever the server offers it — Keycloak (JDBC) and the Antares broker both
connect over TLSv1.3 with no client-side change. Unix-socket clients inside the pod, such
as the instance manager and the metrics exporter, match the `local ... peer` rules and are
unaffected.

## Backups

Continuous WAL archiving and a daily base backup to an S3-compatible object store
(OPS-09, OPS-10). Disabled by default — there is no bucket until someone supplies one, and
a half-configured backup is worse than an honestly absent one. Turn it on per environment:

```yaml
postgres:
  cluster:
    backups:
      enabled: true
      endpointURL: 'https://s3.eu-central-1.example.org'
      destinationPath: 's3://city-backups/postgres'
      region: 'eu-central-1'
      bucket: 'city-backups'
      retentionPolicy: '30d'
      schedule: '0 0 3 * * *'      # CNPG cron has six fields; seconds come first
      existingSecret: 'postgres-backup-s3'
```

WAL and data files are compressed (gzip) and encrypted (AES256) on the way out.

The object-store credentials never appear in these values. `existingSecret` names a Secret
the operator creates with two keys, `ACCESS_KEY_ID` and `ACCESS_SECRET_KEY` — supply it
through SOPS or OpenBao (ADR-N-012, see `components/secrets/README.md`). The chart is
configured with `backups.secret.create: false` so it can never template a credential into
a manifest.

## Point-in-time recovery

Restoring is the operator's decision, never an automatic one (OPS-10, OPS-11). Turning
`recovery.enabled` on bootstraps this cluster from the WAL archive at a chosen instant
instead of from `initdb` — the cluster keeps its name, so every component keeps its
`postgres-cluster-rw` connection string:

```yaml
postgres:
  cluster:
    backups:
      # a fresh path: see below
      destinationPath: 's3://city-backups/postgres-restored-2026-08-15'
    recovery:
      enabled: true
      sourceCluster: 'postgres-cluster'   # the serverName the archive was written under
      targetTime: '2026-08-15T14:30:00Z'  # ISO 8601, quoted; empty = end of the archive
      destinationPath: 's3://city-backups/postgres'
```

The restored cluster carries the source cluster's name, so with the archive left as it is it
would write its own WAL over the history it is reading and there would be nothing to restore
from a second time. The render refuses that: enabling recovery while the backups block still
points at the archive being read stops with an error, and so does a `targetTime` that is not
an ISO 8601 instant. The `recovery-test` environment renders exactly this values change and
`tests/test_postgres_recovery.py` asserts it on every push.

Turn `recovery.enabled` back to `false` once the data is verified, keeping the new
`destinationPath`. Left on, it restores the same old instant again the next time the
`Cluster` object is recreated. The procedure around all of this — stopping the writers,
deleting the damaged cluster, watching the replay — is Runbook 6 in the operations guide.

## Backups in the artifact store (dev)

`postgres.cluster.backups.target: 'artifact-store'` archives to the platform's own artifact store
instead of an external bucket (T-5289); dev runs so. The store's bucket Job creates the bucket
`postgres-backups` and a RustFS user whose policy reaches that bucket alone; its key is the
generated Secret `postgres-backup-artifact-store` in both namespaces, so the database never holds
the store's root key. The values fill in `endpointURL`, `destinationPath` (a path you set inside the
bucket wins, for a restore), `existingSecret` and `encryption: ''`: the store has no KMS and
refuses SSE-S3 ("SSE-S3 requires RUSTFS_SSE_S3_MASTER_KEY"), which would fail every WAL segment.

The store sits on the same node and volume class as the database. That buys point-in-time
recovery, a restore drill with real data and `upgrade.py`'s pre-upgrade backup; it does not survive
the loss of the node. dev keeps 7 days (`retentionPolicy: '7d'`): the store's 10Gi volume holds
the artifacts too. Watch `kubectl exec -n <ns> artifact-store-0 -- df -h /data` after the first week.

The probe reads the newest backup (CNPG `lastSuccessfulBackup`, or the local dump's last success)
and the drill's ConfigMap every five minutes; the status page shows both ages, stale after 26 hours
and 35 days, and `PostgresBackupStale` / `PostgresRestoreDrillStale` alert on the same limits.

## Restore drill

A CronJob (`postgres-cluster-dr-drill`, 04:00 UTC on the 1st, `postgres.databases.drDrill`)
proves the archive restores, to its end and to a point in time (OPS-11, OPS-12, T-0046,
T-5101):

1. It waits while the node has less than `minMemAvailableMiB` available, at most
   `timeoutMinutes`, and otherwise records `skipped`.
2. It counts every table of every database on the source's primary, notes the instant after
   the last count and switches the WAL segment, so that instant reaches the archive at once.
3. It restores the latest base backup and WAL into the throwaway Cluster
   `postgres-cluster-drill` beside the source (one instance, 1 GiB memory limit), and compares
   each database's total rows with the source's: a copy short by more than `tolerancePercent`,
   or missing a database, fails. The copy must also hold an enabled person in an enabled realm
   (`keycloak`) and the Portal's schema at the source's last migration (`portal`,
   `_sqlx_migrations`).
4. It deletes that copy and restores again under the same name, this time to the instant of
   step 2 (`recoveryTarget.targetTime`): the point-in-time restore. Every table must hold the
   count step 2 read, within `tolerancePercent` and at least one row (a commit between a
   table's count and the instant lands in the copy).
5. It records RTO (each restore's time to Ready) and RPO (now minus the last transaction the
   latest restore replayed), as one JSON line (`"event":"dr-drill"`) in its log and in the
   ConfigMap `postgres-cluster-dr-drill` (`result`, `rtoSeconds`, `rpoSeconds`, `detail` for
   the latest restore; `pitrTarget`, `pitrRtoSeconds`, `pitrDetail` for the point in time;
   `at`).
6. It deletes the copy however the drill ends; a copy a crashed drill left is deleted first.

The restores run one after the other, so the node never holds two copies. smartcity (prod)
renders the same CronJob: it drills the day `postgres.cluster.backups` is on there (T-5101).

Without `backups.enabled` there is no archive, and the drill records `no-backup` ("no off-site
backup to drill") and creates nothing. Run it by hand with
`kubectl create job --from=cronjob/postgres-cluster-dr-drill dr-drill-now -n <namespace>`.

The job's Role reads the source Cluster and nothing more, creates and deletes only the copy, and holds
`pods/exec` to their instances: counting rows with the instances' own psql needs no database
password in the job.
