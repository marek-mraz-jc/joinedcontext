# Artifact Store

The S3 API for rendered state (PF-29, PF-30, [Architecture/17]). Authored state lives in Git
and runtime state in PostgreSQL; everything a machine produced from those two and has to serve
fast and unchanged for years lives here: schema artifacts in every formalism, compiled
mappings, RDF dumps, export bundles, app builds and the gateway's cache of file downloads.

The engine is RustFS: one Rust binary, Apache-2.0, plain S3. The platform uses only the S3
subset PF-30 names, so Garage, SeaweedFS, Ceph RGW or a cloud bucket stay a values change away.
Objects are named after the component rather than the engine for the same reason.

Deploy the component with:
```bash
helmfile apply -i --selector component=artifact-store
```

## The bucket, and why the Job exists

Object lock can only be turned on **when a bucket is created** and can never be added
afterwards. The bucket is therefore created by a post-install hook Job rather than by the
first writer:

```
create-bucket --object-lock-enabled-for-bucket     # versioning comes with it
put-object-lock-configuration                      # COMPLIANCE, when a default retention is set
put-bucket-lifecycle-configuration                 # filecache/ expires after a day
```

The Job is idempotent: it runs after every apply, finds the bucket, and re-applies the two
configurations.

`objectLock.defaultRetentionDays` is `0` by default, and that is deliberate. A default
retention locks **every** object in the bucket for that many days, the gateway's file cache
included, and the cache has to stay purgeable. Publishers set COMPLIANCE retention per object
on the prefixes that promise immutability (`schemas/`, `endpoints/`, `dumps/`), which is what
the per-prefix table in Architecture/17 §2 describes. Set a bucket-wide default only for a
store that holds published artifacts and nothing else.

Compliance mode means what it says. Verified against RustFS 1.0.0-rc.5 and again on 1.0.4 (T-5887):

```
An error occurred (AccessDenied) when calling the DeleteObject operation:
Object is under COMPLIANCE retention and cannot be deleted until 2036-09-04
```

A new version of the same key is still accepted; the locked version stays and stays readable,
which is what a versioned schema URL promises a partner (SP-13, DM-26).

## Credentials

Today the store has one credential: `artifact-store-root`, generated into the namespace with
`ACCESS_KEY_ID` and `ACCESS_SECRET_KEY`, read by the store itself and by the bootstrap Job.

The per-organization writer and reader keys of Architecture/17 §3 are **not** here. They are
issued from OpenBao and scoped by bucket policy to one organization's prefixes (ADR-N-012),
which is the reconciler's job; RustFS exposes the admin API it needs
(`/rustfs/admin/v3/add-user`, canned policies, `idp/builtin/policy/attach`) and none of it can
be driven from a Helm chart without shipping a signing client.

Until then the boundary is the network: the store has **no ingress route** and no pre-signed
URLs (PF-32), and its NetworkPolicy accepts port 9000 from exactly three pods — the Context
Gateway, the Portal and the bootstrap Job. Everything else in the cluster is denied, and
nothing outside it can reach the store at all.

## Storage

One node, one volume, `ReadWriteOnce`. Durability is the storage class behind the claim, not
erasure coding, and the store says so at start-up (`storage_class_zero_redundancy`, state
`degraded` — that is the single-drive message, not a fault). An erasure-coded pool across
nodes changes this chart rather than a value.

The process creates its data directory on first start, so the volume must be writable by the
pod's group. With the platform's `securityDefaults` (`runAsUser: 1000`, `fsGroup: 1000`) that
works; a storage class that ignores `fsGroup` leaves the pod in CrashLoop with
`mkdir: cannot create directory '/data/rustfs0': Permission denied`.

Backup is deliberately not configured (OPS-09 scope): every object here is reproducible from
Git, and `jcctl artifacts rebuild --org <o>` is meant to re-render them (Architecture/17 §4;
not implemented yet, so a lost store is rebuilt by re-running the reconciler today). Dumps are
the exception and are mirrored to the database backup bucket when their retention matters.

[Architecture/17]: https://github.com/marek-mraz-jc/joinedcontext-docs/blob/main/Architecture/17-artifact-store.md
