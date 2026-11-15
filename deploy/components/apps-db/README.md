# Apps database

The database of the server WASM Apps (ADR-N-044, AP-149, T-3344): a CloudNativePG cluster of
its own, apart from the platform's, run by the shared operator of the operators layer.

- One schema and two roles per App; the Portal makes them.
- Each shard of `wasm-host` logs in as `wasm_host_<shard>`; `apps-db.cluster.shards` lists the
  shards and must match `wasm-host`'s parts.
- `apps-db.cluster.backups.target: 'artifact-store'` archives WAL and a daily base backup to the
  platform's artifact store under `postgres-backups/apps-db/`, with the key
  `apps-db-backup-artifact-store` that reaches that prefix alone (T-5662; dev does).
- Charts are pinned in `charts.yaml`, images in `images.yaml`, release values in `values/`.

Deploy the component alone with:
$ # navigate to `deployment/` and run:
$ helmfile apply -i --selector component=apps-db
