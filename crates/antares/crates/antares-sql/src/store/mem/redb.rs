// SPDX-License-Identifier: EUPL-1.2
//! `file` mode durability: the redb write-through shadow behind the
//! in-memory maps, its tables and key layout.

use ::redb::{Database, Durability, TableDefinition};
use antares_store::Kind;

// ---- `file` mode: redb write-through shadow --------------------------------
//
// redb is durability only — queries and the matcher keep running on the
// in-memory maps. Every mutation commits to redb (Durability::Immediate,
// fsync) INSIDE the store's write-critical section, so redb apply order is
// exactly memory apply order, and the commit happens before the store call
// returns — i.e. before the HTTP ack (commit-before-ack). Boot rebuilds
// the maps from the file and refuses to start on a format mismatch.
//
// Table per resource family, named after the spec resource, snake_cased.
// The v0 memory store keeps one temporal doc per entity, so `attr_instances`
// has no separate table; entityMaps are TTL-ephemeral and not durable state.
/// A table of the shadow file: byte keys to byte values.
pub(super) type Table = TableDefinition<'static, &'static [u8], &'static [u8]>;

pub(super) const T_ENTITIES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("entities");
pub(super) const T_SUBSCRIPTIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("subscriptions");
pub(super) const T_CSOURCE_REGISTRATIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("csource_registrations");
pub(super) const T_CSOURCE_SUBSCRIPTIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("csource_subscriptions");
pub(super) const T_TEMPORAL_ENTITIES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("temporal_entities");
pub(super) const T_JSONLD_CONTEXTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("jsonld_contexts");
pub(super) const T_SNAPSHOTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("snapshots");
pub(super) const T_ENTITY_MAP_DOCS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("entity_map_docs");
pub(super) const T_DIST_SUBS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("dist_subs");
pub(super) const T_DEAD_LETTERS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("dead_letters");
pub(super) const T_VIEWS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("views");
pub(super) const T_META: TableDefinition<&str, &str> = TableDefinition::new("meta");
/// On-disk format version: bump on any key/value shape change; an older
/// or newer file refuses to load rather than being misread as valid data.
pub(super) const FORMAT_VERSION: &str = "1";

pub(super) fn table_for(kind: Kind) -> Table {
    match kind {
        Kind::Entity => T_ENTITIES,
        Kind::Subscription => T_SUBSCRIPTIONS,
        Kind::Registration => T_CSOURCE_REGISTRATIONS,
        Kind::CSourceSubscription => T_CSOURCE_SUBSCRIPTIONS,
        Kind::Temporal => T_TEMPORAL_ENTITIES,
        Kind::Snapshot => T_SNAPSHOTS,
        Kind::EntityMap => T_ENTITY_MAP_DOCS,
        Kind::DistSub => T_DIST_SUBS,
        Kind::DeadLetter => T_DEAD_LETTERS,
        Kind::View => T_VIEWS,
    }
}

/// Key = `tenant \0 id`. Unambiguous: TenantId is `[A-Za-z0-9_-]{1,128}`
/// by construction, so it can never contain the separator. Takes the tenant
/// as the plain string the maps are keyed by, so a persisted removal can
/// never be skipped for want of a re-parse.
pub(super) fn key_bytes(tenant: &str, id: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(tenant.len().saturating_add(1).saturating_add(id.len()));
    k.extend_from_slice(tenant.as_bytes());
    k.push(0);
    k.extend_from_slice(id.as_bytes());
    k
}

pub(super) fn split_key(key: &[u8]) -> Option<(String, String)> {
    let pos = key.iter().position(|&b| b == 0)?;
    let (tenant, rest) = key.split_at_checked(pos)?;
    Some((
        String::from_utf8(tenant.to_vec()).ok()?,
        String::from_utf8(rest.get(1..)?.to_vec()).ok()?,
    ))
}

pub(super) struct Shadow {
    pub(super) db: Database,
}

impl Shadow {
    /// One txn per mutation, fsynced before return. A failed commit
    /// aborts the process: the alternative is acking writes the file does not
    /// hold, which is the one lie a durable store must never tell.
    /// (Deliberately abort-on-commit-failure; per-request error plumbing only
    /// if a recoverable commit failure mode ever shows up in practice.)
    pub(super) fn write(&self, table: Table, key: &[u8], value: Option<&[u8]>) {
        self.write_many(std::iter::once((table, key, value)));
    }

    /// Every `(table, key, value)` of `writes` in one txn, fsynced once: a
    /// purge or a sweep holds the write lock for one commit, not one per key
    /// (F-R2ANT4-011). `None` deletes the key.
    pub(super) fn write_many<'a>(
        &self,
        writes: impl IntoIterator<Item = (Table, &'a [u8], Option<&'a [u8]>)>,
    ) {
        let result = self.try_write_many(writes);
        if let Err(e) = result {
            tracing::error!("redb commit failed: {e} — aborting: an acked write must be durable");
            std::process::abort();
        }
    }

    /// The commit of [`Self::write_many`], its failure returned.
    fn try_write_many<'a>(
        &self,
        writes: impl IntoIterator<Item = (Table, &'a [u8], Option<&'a [u8]>)>,
    ) -> Result<(), String> {
        let mut tx = self.db.begin_write().map_err(super::text)?;
        tx.set_durability(Durability::Immediate)
            .map_err(super::text)?;
        for (table, key, value) in writes {
            let mut t = tx.open_table(table).map_err(super::text)?;
            match value {
                Some(v) => {
                    t.insert(key, v).map_err(super::text)?;
                }
                None => {
                    t.remove(key).map_err(super::text)?;
                }
            }
        }
        tx.commit().map_err(super::text)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, RwLock};

    /// An in-memory redb file that fails every read after `reads` of them
    /// and every write after `writes`: the I/O errors a disk gives.
    #[derive(Debug)]
    pub(in crate::store::mem) struct Fault {
        pub(in crate::store::mem) bytes: Arc<RwLock<Vec<u8>>>,
        pub(in crate::store::mem) reads: AtomicI64,
        pub(in crate::store::mem) writes: AtomicI64,
    }

    impl Fault {
        pub(in crate::store::mem) fn new(bytes: Vec<u8>, reads: i64, writes: i64) -> Self {
            Self {
                bytes: Arc::new(RwLock::new(bytes)),
                reads: AtomicI64::new(reads),
                writes: AtomicI64::new(writes),
            }
        }

        fn spend(budget: &AtomicI64) -> std::io::Result<()> {
            if budget.fetch_sub(1, Ordering::SeqCst) > 0 {
                Ok(())
            } else {
                Err(std::io::Error::other("injected disk fault"))
            }
        }
    }

    impl ::redb::StorageBackend for Fault {
        fn len(&self) -> std::io::Result<u64> {
            Ok(self.bytes.read().expect("bytes").len() as u64)
        }

        fn read(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
            Self::spend(&self.reads)?;
            let bytes = self.bytes.read().expect("bytes");
            let at = usize::try_from(offset).expect("offset");
            let end = at.checked_add(out.len()).expect("end");
            let past_the_end = std::io::Error::from(std::io::ErrorKind::UnexpectedEof);
            out.copy_from_slice(bytes.get(at..end).ok_or(past_the_end)?);
            Ok(())
        }

        fn set_len(&self, len: u64) -> std::io::Result<()> {
            Self::spend(&self.writes)?;
            let len = usize::try_from(len).expect("len");
            self.bytes.write().expect("bytes").resize(len, 0);
            Ok(())
        }

        fn sync_data(&self) -> std::io::Result<()> {
            Self::spend(&self.writes)
        }

        fn write(&self, offset: u64, data: &[u8]) -> std::io::Result<()> {
            Self::spend(&self.writes)?;
            let mut bytes = self.bytes.write().expect("bytes");
            let at = usize::try_from(offset).expect("offset");
            let end = at.checked_add(data.len()).expect("end");
            let len = bytes.len().max(end);
            bytes.resize(len, 0);
            bytes
                .get_mut(at..end)
                .expect("resized")
                .copy_from_slice(data);
            Ok(())
        }
    }

    /// A write that the disk refuses at any step returns its error instead of
    /// committing part of it; with a disk that answers it commits.
    #[test]
    fn a_refused_write_returns_its_error() {
        let mut failed = std::collections::BTreeSet::new();
        let mut committed = false;
        for writes in 0..200 {
            let Ok(db) =
                Database::builder().create_with_backend(Fault::new(Vec::new(), i64::MAX, writes))
            else {
                continue;
            };
            let shadow = Shadow { db };
            let doc: &[u8] = b"{}";
            let rows = [
                (T_ENTITIES, &b"t\0a"[..], Some(doc)),
                (T_VIEWS, &b"t\0v"[..], Some(doc)),
                (T_ENTITIES, &b"t\0a"[..], None),
            ];
            match shadow.try_write_many(rows) {
                Ok(()) => committed = true,
                Err(e) => {
                    failed.insert(e);
                }
            }
        }
        assert!(committed, "a disk with room commits");
        assert!(!failed.is_empty(), "no injected fault reached the write");
    }

    /// A table that the file holds under another key/value type refuses the
    /// write with the error instead of committing it.
    #[test]
    fn a_table_of_another_type_refuses_the_write() {
        let db = Database::builder()
            .create_with_backend(Fault::new(Vec::new(), i64::MAX, i64::MAX))
            .expect("in-memory db");
        let tx = db.begin_write().expect("tx");
        let other: TableDefinition<&str, &str> = TableDefinition::new("entities");
        tx.open_table(other)
            .expect("table")
            .insert("k", "v")
            .expect("row");
        tx.commit().expect("commit");
        let shadow = Shadow { db };
        let err = shadow
            .try_write_many([(T_ENTITIES, &b"t\0a"[..], Some(&b"{}"[..]))])
            .expect_err("refused");
        assert!(!err.is_empty());
    }

    /// A read the disk refuses while a write commit opens its tables and
    /// walks their rows returns its error; with a disk that answers it commits.
    #[test]
    fn a_refused_read_returns_its_error() {
        let first = Fault::new(Vec::new(), i64::MAX, i64::MAX);
        let bytes = first.bytes.clone();
        let db = Database::builder()
            .create_with_backend(first)
            .expect("in-memory db");
        let doc: &[u8] = b"{}";
        let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("t\0e{i}").into_bytes()).collect();
        let seed: Vec<_> = keys
            .iter()
            .flat_map(|k| [(T_ENTITIES, k.as_slice(), Some(doc))])
            .chain([(T_VIEWS, &b"t\0v"[..], Some(doc))])
            .collect();
        Shadow { db }.try_write_many(seed).expect("seed commits");
        let file = bytes.read().expect("bytes").clone();
        let mut failed = std::collections::BTreeSet::new();
        let mut committed = false;
        for reads in 0..300 {
            // no page cache: every page the commit opens or walks is a read of the disk
            let Ok(db) = Database::builder()
                .set_cache_size(0)
                .create_with_backend(Fault::new(file.clone(), reads, i64::MAX))
            else {
                continue;
            };
            let rows = [
                (T_VIEWS, &b"t\0v"[..], None),
                (T_ENTITIES, keys[150].as_slice(), Some(&b"[]"[..])),
                (T_ENTITIES, keys[7].as_slice(), None),
            ];
            match (Shadow { db }).try_write_many(rows) {
                Ok(()) => committed = true,
                Err(e) => {
                    failed.insert(e);
                }
            }
        }
        assert!(committed, "a disk with room commits");
        assert!(!failed.is_empty(), "a refused read fails the write");
        assert!(failed.len() < 300, "the faults are errors, not panics");
    }
}
