// SPDX-License-Identifier: EUPL-1.2
//! The storage seam: two driver traits every backend implements, plus the
//! backend-neutral types they speak (resource kinds, filter shapes, the
//! store-mode enum). This crate names no backend — redb, sqlx and the
//! browser's OPFS all live behind these traits in their own crates.
//!
//! Current-state and temporal storage are SEPARATE interfaces on purpose:
//! a deployment may run postgres current-state with no temporal store at
//! all, or memory current-state with a database-backed history. A driver
//! that does not support an operation answers with an error (or a benign
//! no-op for internal bookkeeping), never a panic — `NoTemporal` is the
//! canonical instance.
#![cfg_attr(not(test), warn(clippy::expect_used))]
#![deny(missing_docs)]

#[cfg(feature = "test-kit")]
/// The shared driver contract every backend runs as a test. It asserts, so
/// it panics on a store that breaks an invariant — that is the point.
#[allow(clippy::expect_used)]
pub mod contract;
pub mod filter;

use antares_model::{NgsiError, TenantId};
use serde_json::Value;

/// [`CurrentStateDriver::list_digests`] of documents read out: each one's
/// `id` and a digest of its JSON text, a document without an id left out.
#[must_use]
pub fn digests(docs: Vec<Value>) -> Vec<(String, u64)> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    docs.into_iter()
        .filter_map(|doc| {
            let id = doc.get("id")?.as_str()?.to_owned();
            let mut h = DefaultHasher::new();
            doc.to_string().hash(&mut h);
            Some((id, h.finish()))
        })
        .collect()
}

/// A stored document as the object it is. Every document a driver holds is a
/// JSON object — `contract` asserts the shape on every backend — so a value
/// that is not one means the driver returned something the contract forbids
/// underneath a live request. That fails the one request; it never takes the
/// process down, which is what an unwrap here would do.
pub fn stored_object(doc: &mut Value) -> Result<&mut serde_json::Map<String, Value>, NgsiError> {
    doc.as_object_mut()
        .ok_or_else(|| NgsiError::InternalError("stored document is not a JSON object".into()))
}

/// What a [`ChangeHook`] call returns: the hook records temporal history
/// through the temporal driver, which is asynchronous, so the driver that
/// fires the hook awaits it before its own write returns.
pub type HookFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

/// Called with (tenant, before, after) on every entity write — the local-mode
/// change feed: create ⇒ (None, Some), delete ⇒ (Some, None). Shared rather
/// than owned: a driver clones it out of its lock before awaiting, since a
/// lock guard cannot be held across an await.
pub type ChangeHook = std::sync::Arc<
    dyn for<'a> Fn(&'a TenantId, Option<Value>, Option<Value>) -> HookFuture<'a> + Send + Sync,
>;

/// Which resource family an operation touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 5.2.4 Entity current-state documents.
    Entity,
    /// 5.2.12 Subscription documents.
    Subscription,
    /// 5.2.9 Context Source Registration documents.
    Registration,
    /// Context Source Registration Subscription documents (5.11).
    CSourceSubscription,
    /// Temporal Representation documents (5.2.5).
    Temporal,
    /// 5.16 Snapshot status documents (+ the internal synth-tenant index).
    Snapshot,
    /// 5.14 EntityMap API documents.
    EntityMap,
    /// 5.8.1.4 distributed-subscription mappings (remote ids per CSR).
    DistSub,
    /// Notifications a delivery policy gave up on (dead letters), kept
    /// under the subscription's tenant for replay.
    DeadLetter,
    /// Endpoint views (ADR-0023 rule 10): configuration the `views` build
    /// serves, under the reserved tenant `antares-views`, and the policy
    /// engine's rule documents (ADR-0020 amendment) under `antares-policy`.
    /// No NGSI-LD route reads or writes this kind.
    View,
}

/// The four store backends, decided ONCE at startup and threaded as a value —
/// never re-derived from strings or runtime probes, so a section gated on the
/// wrong mode is unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StoreMode {
    /// In-process maps; nothing survives a restart.
    #[default]
    Memory,
    /// Embedded single-file store on local disk.
    File,
    /// PostgreSQL with PostGIS.
    Postgres,
    /// PostgreSQL with PostGIS and TimescaleDB hypertables for history.
    Timescale,
}

impl StoreMode {
    /// Every backend this workspace knows, in the order a listing shows
    /// them. One source of truth: `FromStr`, the unknown-mode message and
    /// the broker's built-with shelf all read it, so a backend added to the
    /// enum cannot go missing from any of them.
    pub const ALL: [StoreMode; 4] = [
        StoreMode::Memory,
        StoreMode::File,
        StoreMode::Postgres,
        StoreMode::Timescale,
    ];

    /// The accepted mode names, `memory|file|postgres|timescale`.
    pub fn names() -> String {
        StoreMode::ALL
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }

    /// The mode name as accepted by `FromStr` (`memory|file|postgres|timescale`).
    pub fn as_str(self) -> &'static str {
        match self {
            StoreMode::Memory => "memory",
            StoreMode::File => "file",
            StoreMode::Postgres => "postgres",
            StoreMode::Timescale => "timescale",
        }
    }
    /// Shared-database modes — the only ones that can back multiple instances.
    pub fn is_pg(self) -> bool {
        matches!(self, StoreMode::Postgres | StoreMode::Timescale)
    }
}

impl std::str::FromStr for StoreMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        StoreMode::ALL
            .into_iter()
            .find(|m| m.as_str() == s)
            .ok_or_else(|| format!("unknown store mode {s} ({})", StoreMode::names()))
    }
}

impl std::fmt::Display for StoreMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one tenant holds, per current-state document kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenantStats {
    /// The tenant name.
    pub tenant: String,
    /// When the tenant row was created, if the backend records it.
    pub created_at: Option<String>,
    /// 5.2.4 entities.
    pub entities: u64,
    /// 5.2.12 subscriptions.
    pub subscriptions: u64,
    /// 5.2.9 context source registrations.
    pub registrations: u64,
    /// Context source registration subscriptions.
    pub csource_subscriptions: u64,
    /// 5.16 snapshots.
    pub snapshots: u64,
    /// Entity maps (5.14).
    pub entity_maps: u64,
    /// Distributed subscriptions this tenant placed at other brokers.
    pub dist_subs: u64,
}

/// How many bytes one tenant's Entities take in the store (T-2890), measured
/// there and never estimated: the stored row on Postgres, the serialized
/// document in memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenantBytes {
    /// Every Entity of the tenant.
    pub entities: u64,
    /// The same bytes by the Entity's first type, so the values sum to
    /// `entities`.
    pub by_type: std::collections::BTreeMap<String, u64>,
}

/// The read-modify-write closure as the object-safe seam carries it. The
/// typed `Result<T, E>` travels through [`CurrentStateDriverExt::mutate`]'s
/// side slot; the boxed closure only signals commit (`Ok`) vs reject
/// (`Err`) to the driver.
pub type MutateFn<'a> = Box<dyn FnOnce(&mut Value) -> Result<(), ()> + Send + 'a>;
/// Per-id variant for batch mutation; the driver calls it once per PRESENT
/// id, in input order — the ext trait's error slot depends on that order.
pub type BatchMutateFn<'a> = Box<dyn FnMut(&str, &mut Value) -> Result<(), ()> + Send + 'a>;
/// The delivery stamp expressed as a `mutate`. This is the rule itself —
/// what `timesSent`, `lastNotification`, `lastSuccess` and `status` become
/// after one attempt (5.2.14.2) — so a backend that reimplements
/// [`CurrentStateDriver::record_delivery`] in its own query language is
/// reimplementing THIS, and the two must agree.
pub async fn record_delivery_via_mutate(
    d: &(impl CurrentStateDriver + ?Sized),
    tenant: &TenantId,
    kind: Kind,
    id: &str,
    now: &str,
) -> Result<Option<Delivery>, NgsiError> {
    let mut out: Option<Delivery> = None;
    d.mutate::<(), ()>(tenant, kind, id, |doc| {
        let mut prev_success = None;
        if let Some(o) = doc.as_object_mut() {
            o.remove("status");
        }
        if let Some(n) = doc
            .as_object_mut()
            .and_then(|o| o.get_mut("notification"))
            .and_then(Value::as_object_mut)
        {
            let sent = n.get("timesSent").and_then(Value::as_i64).unwrap_or(0);
            n.insert("timesSent".into(), serde_json::json!(sent + 1));
            n.insert("lastNotification".into(), Value::String(now.to_owned()));
            prev_success = n.insert("lastSuccess".into(), Value::String(now.to_owned()));
            n.insert("status".into(), Value::String("ok".into()));
        }
        out = Some(Delivery {
            doc: doc.clone(),
            prev_success,
        });
        Ok(())
    })
    .await?;
    Ok(out)
}

/// The failure stamp expressed as a `mutate` (5.8.6 / 5.11.7 after a failed
/// attempt): `status` "failed" on the document and its notification, the
/// optimistic `lastSuccess` rolled back to `prev_success`, `lastFailure` set
/// to `now` and `timesFailed` moved by one. The rule itself, as
/// [`record_delivery_via_mutate`] is for a success: a backend that
/// reimplements [`CurrentStateDriver::record_failure`] must agree with it.
pub async fn record_failure_via_mutate(
    d: &(impl CurrentStateDriver + ?Sized),
    tenant: &TenantId,
    kind: Kind,
    id: &str,
    mut prev_success: Option<Value>,
    now: &str,
) -> Result<Option<Value>, NgsiError> {
    let mut out: Option<Value> = None;
    d.mutate::<(), ()>(tenant, kind, id, |doc| {
        if let Some(o) = doc.as_object_mut() {
            o.insert("status".into(), Value::String("failed".into()));
        }
        if let Some(n) = doc
            .as_object_mut()
            .and_then(|o| o.get_mut("notification"))
            .and_then(Value::as_object_mut)
        {
            match prev_success.take() {
                Some(v) => n.insert("lastSuccess".into(), v),
                None => n.remove("lastSuccess"),
            };
            n.insert("lastFailure".into(), Value::String(now.to_owned()));
            // Table 5.2.14.2-1 timesFailed: "Number of times an
            // unsuccessful response (or timeout) has been received when
            // delivering the notification" — an output-only member
            // implementations shall generate.
            let failed = n.get("timesFailed").and_then(Value::as_i64).unwrap_or(0);
            n.insert(
                "timesFailed".into(),
                serde_json::json!(failed.saturating_add(1)),
            );
            n.insert("status".into(), Value::String("failed".into()));
        }
        out = Some(doc.clone());
        Ok(())
    })
    .await?;
    Ok(out)
}

/// The forward stamp expressed as a `mutate`. This is the Table 5.2.9-2
/// rule itself — what `timesSent`, `timesFailed`, `lastSuccess`,
/// `lastFailure` and `status` become after one distributed operation — so a
/// backend that reimplements [`CurrentStateDriver::record_forward`] in its
/// own query language is reimplementing THIS, and the two must agree.
///
/// `timesSent` moves on EVERY attempt: the table defines it as the number of
/// times the registration "triggered a distributed operation, including
/// failed attempts". A member is written when it first has meaning, which is
/// what cardinality 0..1 and "created on first successful operation" ask for:
/// a registration that has only ever succeeded carries no `timesFailed` and
/// no `lastFailure`, and one that has only ever failed carries no
/// `lastSuccess`. `status` always names the LAST attempt.
pub async fn record_forward_via_mutate(
    d: &(impl CurrentStateDriver + ?Sized),
    tenant: &TenantId,
    id: &str,
    now: &str,
    ok: bool,
) -> Result<Option<Value>, NgsiError> {
    let mut out: Option<Value> = None;
    d.mutate::<(), ()>(tenant, Kind::Registration, id, |doc| {
        if let Some(o) = doc.as_object_mut() {
            let sent = o.get("timesSent").and_then(Value::as_i64).unwrap_or(0);
            o.insert("timesSent".into(), serde_json::json!(sent + 1));
            if ok {
                o.insert("lastSuccess".into(), Value::String(now.to_owned()));
                o.insert("status".into(), Value::String("ok".into()));
            } else {
                let failed = o.get("timesFailed").and_then(Value::as_i64).unwrap_or(0);
                o.insert("timesFailed".into(), serde_json::json!(failed + 1));
                o.insert("lastFailure".into(), Value::String(now.to_owned()));
                o.insert("status".into(), Value::String("failed".into()));
            }
        }
        out = Some(doc.clone());
        Ok(())
    })
    .await?;
    Ok(out)
}

/// What one delivery attempt wrote: the stored subscription as it now
/// stands (the mirror is fed from it) and the `lastSuccess` that was there
/// before, which a failed attempt puts back.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// The subscription as it now stands, the shape the mirror is fed.
    pub doc: Value,
    /// The `notification.lastSuccess` this attempt overwrote, absent when
    /// the subscription had never succeeded.
    pub prev_success: Option<Value>,
}

/// ADR-0021: the Tenant a stored `@context` row belongs to, or `None` when it
/// belongs to none.
///
/// 5.13.1 "Cached" is a copy of a public document the broker downloaded for
/// whoever named its URL, so every Tenant reaches it. "Hosted" and
/// "ImplicitlyCreated" are documents a client stored THROUGH a Tenant, and
/// 5.5.7 makes their term mappings decide what that Tenant's payloads mean;
/// the owner travels in the row. A row written before the member existed
/// reads as the default Tenant's, and so does a row whose `kind` is missing
/// altogether: no writer produces one, and an unrecognised row is safer read
/// as somebody's than as everybody's.
pub fn context_row_owner(row: &Value) -> Option<&str> {
    (row["kind"].as_str() != Some("Cached"))
        .then(|| row["owner"].as_str().unwrap_or(TenantId::DEFAULT))
}

/// Whether a call acting for `tenant` — `None` for no Tenant at all — reaches
/// the row. The rule [`context_row_owner`] states, in the form the stores and
/// the API both need.
pub fn context_row_visible(row: &Value, tenant: Option<&TenantId>) -> bool {
    match context_row_owner(row) {
        None => true,
        Some(owner) => tenant.is_some_and(|t| t.as_str() == owner),
    }
}

/// Current-state storage: everything except the temporal evolution.
///
/// Contract carried over from the enum seam it replaces: every mutate is
/// one transaction under the row lock, and a missing row is `None`, never
/// an insert (a bookkeeping writeback racing a DELETE must not resurrect
/// the row). Backends map their internal failures to
/// `NgsiError::InternalError` with a GENERIC client-visible detail.
#[async_trait::async_trait]
pub trait CurrentStateDriver: Send + Sync {
    /// Readiness RIGHT NOW — a lost database flips /q/ready to 503.
    async fn ping(&self) -> Result<(), NgsiError>;
    /// Liveness: true when the driver can no longer serve and only a
    /// process restart helps (a leaked connection pool). The default never
    /// is.
    async fn wedged(&self) -> bool {
        false
    }
    /// (Queued writers, peak) of a single-writer commit section, if the
    /// backend has one.
    fn commit_queue(&self) -> Option<(usize, usize)> {
        None
    }
    /// Drain: finish in-flight work and disconnect cleanly.
    async fn close(&self);
    /// What this driver runs on, for `/q/health`: engine, server version,
    /// extensions — whatever an operator needs to tell two deployments of
    /// the same backend name apart. Read from state captured at startup,
    /// never by querying on the request: health is polled. An empty object
    /// (the default) means the driver has nothing to add to its name.
    fn version_info(&self) -> Value {
        Value::Object(serde_json::Map::new())
    }
    /// Installs the (tenant, before, after) hook called on every entity write.
    fn set_change_hook(&self, h: ChangeHook);
    /// Turn the same-transaction outbox producer on (bus=nats).
    fn set_outbox(&self, on: bool) {
        let _ = on;
    }
    /// Outbox drain: oldest-first page of pending rows `(seq, tenant, event)`.
    async fn outbox_peek(&self, limit: i64) -> Result<Vec<(i64, String, Value)>, NgsiError> {
        let _ = limit;
        Ok(Vec::new())
    }
    /// Outbox drain: delete EXACTLY the published rows.
    async fn outbox_ack(&self, seqs: &[i64]) -> Result<u64, NgsiError> {
        let _ = seqs;
        Ok(0)
    }
    /// Outbox drain: keep the rows of events the bus could not carry whole
    /// and take them out of the drain's page. The published message holds a
    /// claim-check reference; the row holds the bodies it stands for.
    async fn outbox_retain(&self, tenant: &TenantId, seqs: &[i64]) -> Result<u64, NgsiError> {
        let _ = (tenant, seqs);
        Ok(0)
    }
    /// The whole event behind a claim-check reference, or `None` once it has
    /// been reaped. Scoped to the tenant that wrote it.
    async fn outbox_event(&self, seq: i64, tenant: &TenantId) -> Result<Option<Value>, NgsiError> {
        let _ = (seq, tenant);
        Ok(None)
    }

    /// Insert a document; `false` if the id already exists (nothing written).
    async fn create(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError>;
    /// Batch create (entities only); created-flags in input order.
    async fn batch_create(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError>;
    /// Batch delete (entities only); deleted-flags in input order, a
    /// duplicate id deletes once and reads absent the second time.
    async fn batch_delete(&self, tenant: &TenantId, ids: &[String])
        -> Result<Vec<bool>, NgsiError>;
    /// Batch upsert with REPLACE semantics (entities only); created-flags in
    /// input order.
    async fn batch_upsert(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError>;
    /// Insert or replace a document. `true` means a document was ALREADY
    /// there and this call replaced it; `false` means this call created it.
    /// The polarity is the opposite of [`Self::batch_upsert`], which answers
    /// created-flags — the batch path needs them to split 201 from 204
    /// (5.6.8) while the single path ignores the value.
    async fn upsert(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError>;
    /// Read one document; `None` if absent.
    async fn get(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
    ) -> Result<Option<Value>, NgsiError>;
    /// Several Entities at once, keyed by id; an absent one has no entry. A
    /// backend that can read them together overrides this (Postgres: one
    /// transaction, T-4085); the default reads them one by one.
    async fn get_entities(
        &self,
        tenant: &TenantId,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Value>, NgsiError> {
        let mut found = std::collections::HashMap::with_capacity(ids.len());
        for id in ids {
            if let Some(doc) = self.get(tenant, Kind::Entity, id).await? {
                found.insert((*id).to_owned(), doc);
            }
        }
        Ok(found)
    }
    /// Delete one document; `false` if it was absent.
    async fn delete(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<bool, NgsiError>;
    /// Delete one Entity, but only if `keep` accepts the stored document.
    /// `false` = absent OR refused; the caller cannot tell the two apart and
    /// does not need to, because 5.6.6.4 gives the same answer to both: an
    /// Entity the selector excludes "is not known" for this operation.
    ///
    /// The read and the delete happen under one lock. Required, deliberately:
    /// the obvious default is `get` then `delete`, and that pair is the race
    /// this method exists to remove — between the two the Entity can be
    /// deleted and recreated under the same id, and the delete then lands on
    /// a document the caller never inspected. A backend inheriting a default
    /// would keep the race with no compile error to say so.
    async fn delete_entity_if(
        &self,
        tenant: &TenantId,
        id: &str,
        keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
    ) -> Result<bool, NgsiError>;
    /// Every document of this kind in the tenant.
    ///
    /// A backend may refuse a tenant that holds too many to materialize
    /// (5.5.6 TooManyResults). That is right for a client query and wrong
    /// for a reader that must see ALL of them — use [`Self::list_page`].
    async fn list(&self, tenant: &TenantId, kind: Kind) -> Result<Vec<Value>, NgsiError>;
    /// Every document of this kind in the tenant as its id and a digest of
    /// its body, for a reader that holds the bodies already and fetches only
    /// the ones whose digest changed (T-4354). Two equal bodies have one
    /// digest on one backend; nothing else is promised of it. Refused as
    /// [`Self::list`] is. A backend that digests without reading the
    /// documents out overrides this; the default reads them, which is
    /// correct and no cheaper.
    async fn list_digests(
        &self,
        tenant: &TenantId,
        kind: Kind,
    ) -> Result<Vec<(String, u64)>, NgsiError> {
        Ok(digests(self.list(tenant, kind).await?))
    }
    /// One id-ordered page of documents, for the internal readers that must
    /// see every one of them and so cannot be refused: ids strictly greater
    /// than `after`, at most `limit`. A short page means the end.
    ///
    /// Keyset, not offset: the caller walks a tenant that is being written
    /// to underneath it, where OFFSET skips and repeats rows. The peak cost
    /// is one page, not the whole tenant, which is what the row ceiling on
    /// `list` was protecting — so this carries no ceiling of its own, and a
    /// backend may not refuse it for volume.
    ///
    /// Required, deliberately: the obvious default is `list` sliced, and
    /// `list` is the read that may refuse. A backend inheriting that would
    /// silently reacquire the outage this method exists to prevent, with no
    /// compile error to say so.
    async fn list_page(
        &self,
        tenant: &TenantId,
        kind: Kind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, NgsiError>;
    /// One id-ordered window of documents and the size of the whole set:
    /// elements `offset..offset + limit`, plus the count 6.3.10's `count`
    /// parameter reports. `limit` 0 is a legal request for the count alone.
    ///
    /// 5.5.9.1: "the query resolution mechanisms of the NGSI-LD System shall
    /// ensure that only up to a maximum of L NGSI-LD Elements are RETRIEVED
    /// and returned to the NGSI-LD client". Reading a whole tenant to serve
    /// one page of it is what that sentence rules out, and it is why this
    /// carries no row ceiling: the window bounds the result by construction,
    /// so the only thing a ceiling could refuse is a page the client is
    /// entitled to.
    ///
    /// Offset, not the keyset of [`Self::list_page`], because this serves
    /// 5.5.9.2's `limit`/`offset`, which lets a client "jump to a desired
    /// set of elements". A cursor cannot answer that; the two reads exist
    /// for different callers and neither replaces the other.
    ///
    /// Required for the same reason `list_page` is: the obvious default
    /// slices `list`, and `list` is the read that may refuse.
    async fn list_slice(
        &self,
        tenant: &TenantId,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Value>, usize), NgsiError>;
    /// Registrations that can match these ids/types (a backend may narrow;
    /// returning the full tenant list is always correct).
    async fn matching_registrations(
        &self,
        tenant: &TenantId,
        ids: Option<&[String]>,
        types: Option<&[String]>,
    ) -> Result<Vec<Value>, NgsiError>;
    /// Query Entities with the filter pushed down where the backend can
    /// take it; the caller re-checks unless the outcome says `decided`.
    async fn query_entities(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome, NgsiError>;
    /// `query_entities` for a caller that re-decides every row and walks the
    /// match set in id-ordered pages: the backend may push `f.page` over the
    /// candidates it cannot decide (`paged` without `decided`), and never
    /// counts them. The default reads as `query_entities` does.
    async fn query_candidates(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome, NgsiError> {
        self.query_entities(tenant, f).await
    }
    /// `query_entities` with each row as its JSON text (T-6055): a page of
    /// 1000 stored Entities is ~5x larger as `Value` trees than as text, and
    /// a caller that decides and writes one Entity at a time holds the page
    /// as text. The default writes each document out.
    async fn query_entities_text(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome<Box<serde_json::value::RawValue>>, NgsiError> {
        let o = self.query_entities(tenant, f).await?;
        Ok(filter::QueryOutcome {
            rows: o
                .rows
                .into_iter()
                .map(|d| serde_json::value::to_raw_value(&d))
                .collect::<Result<_, _>>()
                .map_err(|e| NgsiError::InternalError(e.to_string()))?,
            decided: o.decided,
            paged: o.paged,
            total: o.total,
        })
    }
    /// The Entity types the Tenant's live Entities carry (5.7.5), sorted,
    /// read without the documents: `/types` names them, and folding every
    /// Entity for the names read a whole Tenant per call (T-4610). `None`
    /// when the backend has no such read, and the caller folds instead.
    async fn entity_type_names(&self, tenant: &TenantId) -> Result<Option<Vec<String>>, NgsiError> {
        let _ = tenant;
        Ok(None)
    }
    /// The types the Tenant's live Entities of two or more types carry when
    /// they also carry one of `any_of`, sorted (T-4683): a view's rule names
    /// its types, and a type the rule does not name is shown only on an
    /// Entity of both. An Entity of `any_of` alone adds nothing. `None` when
    /// the backend has no such read, and the caller probes every type.
    async fn entity_types_beside(
        &self,
        tenant: &TenantId,
        any_of: &[String],
    ) -> Result<Option<Vec<String>>, NgsiError> {
        let _ = (tenant, any_of);
        Ok(None)
    }
    /// Each distinct `lead + s` the Tenant's ids start with, `s` being the
    /// text after `lead` up to the first `sep` (an id without one stands
    /// for itself), sorted by bytes (T-5661): with
    /// [`filter::id_pattern_segments`] they turn an id pattern into exact
    /// prefixes. `None` when the backend has no such read or more than
    /// `max` exist.
    async fn entity_id_segments(
        &self,
        tenant: &TenantId,
        lead: &str,
        sep: char,
        max: usize,
    ) -> Result<Option<Vec<String>>, NgsiError> {
        let _ = (tenant, lead, sep, max);
        Ok(None)
    }
    /// The (type, member name) pairs the Tenant's Entities carry, sorted
    /// (T-4934): `/types` with details and `/attributes` probe these instead
    /// of folding every Entity. The list may name a pair no live Entity
    /// carries any more, never miss one a live Entity carries; member names
    /// include the Entity frame's. `None` when the backend keeps no such
    /// list, and the caller folds instead.
    async fn entity_attr_pairs(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<Vec<(String, String)>>, NgsiError> {
        let _ = tenant;
        Ok(None)
    }
    /// Read-modify-write under the row lock; `None` = absent (never an
    /// insert), `Some(Err)` = the closure rejected, nothing committed.
    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError>;
    /// Batch read-modify-write (entities only); results align with `ids`.
    async fn batch_mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        f: BatchMutateFn<'a>,
    ) -> Result<Vec<Option<Result<(), ()>>>, NgsiError>;
    /// 5.2.14.2 delivery bookkeeping: stamp one delivery attempt on a
    /// subscription and hand back the stored document. `timesSent` moves by
    /// one, `lastNotification` and `lastSuccess` take `now`, and the previous
    /// `lastSuccess` comes back so a failed attempt can roll it back.
    /// `None` means the row is gone — the subscription was deleted between
    /// matching and delivery, and nothing may be sent (5.8.6).
    ///
    /// The default expresses it as a `mutate`, which is correct everywhere.
    /// A backend whose `mutate` locks the row across a network round trip
    /// should override it with one statement: at fan-out every delivery on
    /// one subscription contends for that row, so the lock hold time — not
    /// the statement count — is what serializes delivery.
    async fn record_delivery(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        now: &str,
    ) -> Result<Option<Delivery>, NgsiError> {
        record_delivery_via_mutate(self, tenant, kind, id, now).await
    }
    /// Book one failed attempt (see [`record_failure_via_mutate`]): the
    /// stored document as it now stands, `None` when the row is gone. The
    /// default is that `mutate`; a backend whose `mutate` holds the row lock
    /// across a round trip should write it as one statement (T-6606).
    async fn record_failure(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        prev_success: Option<Value>,
        now: &str,
    ) -> Result<Option<Value>, NgsiError> {
        record_failure_via_mutate(self, tenant, kind, id, prev_success, now).await
    }
    /// [`Self::record_delivery`] for several attempts at once, one answer per
    /// attempt in input order. An attempt is (id, instant, times): the ids
    /// are distinct, `times` notifications are booked at the instant (the
    /// last of a merged window, T-6371) and the pre-image is the value it
    /// overwrote. The default books them one by one; a backend that can
    /// should write them as one statement, which is what keeps a fan-out of
    /// thousands of in-process deliveries at one round trip per batch
    /// (T-5159).
    async fn record_deliveries(
        &self,
        tenant: &TenantId,
        kind: Kind,
        attempts: &[(&str, &str, i64)],
    ) -> Result<Vec<Option<Delivery>>, NgsiError> {
        let mut out = Vec::with_capacity(attempts.len());
        for (id, now, times) in attempts {
            let mut booked = None;
            for _ in 0..*times {
                booked = self.record_delivery(tenant, kind, id, now).await?;
            }
            out.push(booked);
        }
        Ok(out)
    }
    /// Reap expired docs/instances (backends with their own maintenance job
    /// return 0).
    async fn sweep_expired(&self) -> usize {
        0
    }
    /// Retention of one Entity type in one Tenant (`/q/retention`, T-3900):
    /// delete at most `batch` Entities whose types include `type_iri`, that
    /// carry no `expiresAt` of their own (a writer's 4.22 stamp wins) and
    /// were last modified before `before`, a UTC DateTime. Returns how many
    /// were deleted, or with `dry_run` would be, deleting nothing; a driver
    /// without current state has none.
    async fn retain_current(
        &self,
        tenant: &TenantId,
        type_iri: &str,
        before: &str,
        batch: usize,
        dry_run: bool,
    ) -> Result<u64, NgsiError> {
        let _ = (tenant, type_iri, before, batch, dry_run);
        Ok(0)
    }
    /// The Entities, across every Tenant, that carry a [`HISTORY_RETENTION`]
    /// Property (DM-97): at most `limit` of (tenant, id, the Property as
    /// stored). Cross-tenant maintenance, like the 4.22 reap; a driver
    /// without current state has none.
    async fn history_overrides(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, Value)>, NgsiError> {
        let _ = limit;
        Ok(Vec::new())
    }
    /// Table 5.2.9-2 forward bookkeeping: stamp one distributed operation on
    /// a registration and hand back the stored document. `None` means the row
    /// is gone — the registration was deleted while its forward was in
    /// flight, and a bookkeeping writeback must never resurrect it.
    ///
    /// The default expresses it as a `mutate`, which is correct everywhere.
    // ponytail: one read-modify-write per forward, so a registration every
    // request fans out to is one contended row; a backend whose `mutate`
    // locks that row across a network round trip can collapse it into one
    // statement the way `record_delivery` does, once a profile asks.
    async fn record_forward(
        &self,
        tenant: &TenantId,
        id: &str,
        now: &str,
        ok: bool,
    ) -> Result<Option<Value>, NgsiError> {
        record_forward_via_mutate(self, tenant, id, now, ok).await
    }
    /// 5.5.10: the default Tenant implicitly exists; others once created.
    async fn tenant_exists(&self, tenant: &TenantId) -> Result<bool, NgsiError>;
    /// The iteration domain of the interval sweep and of every mirror
    /// hydration: a tenant holding a Subscription, a Context Source
    /// Registration Subscription OR a Registration SHALL appear. A superset
    /// is allowed — the callers list per tenant afterwards and an empty list
    /// costs nothing — so a backend that cannot narrow the set cheaply may
    /// return every tenant it knows. A SUBSET is a silent outage: a tenant
    /// missing here never fires a periodic notification and never reaches
    /// the mirror.
    ///
    /// Registrations are in the domain because one of the hydrations fills
    /// the REGISTRATION mirror, and the federation path reads that mirror
    /// alone whenever it is installed. A domain that stopped at
    /// subscription-holding tenants left a tenant with registrations and no
    /// subscription forwarding to no Context Source at all.
    async fn subscription_tenants(&self) -> Result<Vec<String>, NgsiError>;
    /// Every tenant the backend knows, sorted. The default Tenant is listed
    /// even when empty (5.5.10: it always exists). Names only: at the
    /// 10 000-tenant target (ADR-0001) an inventory carrying per-kind counts
    /// would cost a count per kind per tenant, so the counts are paid per
    /// lookup in `tenant_stats_one`.
    async fn tenant_ids(&self) -> Result<Vec<String>, NgsiError> {
        Err(NgsiError::OperationNotSupported("tenant inventory".into()))
    }
    /// What one tenant holds; `None` when it does not exist.
    async fn tenant_stats_one(&self, tenant: &TenantId) -> Result<Option<TenantStats>, NgsiError> {
        let _ = tenant;
        Err(NgsiError::OperationNotSupported("tenant inventory".into()))
    }
    /// The bytes one tenant's Entities take; a scan of the tenant, so the
    /// caller caches it.
    async fn tenant_bytes(&self, tenant: &TenantId) -> Result<TenantBytes, NgsiError> {
        let _ = tenant;
        Err(NgsiError::OperationNotSupported("tenant size".into()))
    }
    /// The store's statistics' guess at [`Self::tenant_bytes`], for a read
    /// whose exact scan failed or ran out of time (T-6159); the answer says
    /// it is estimated. `entities` is the tenant's Entity count the caller
    /// already holds, the guess's fallback when no split by type can be
    /// counted in time. A driver without statistics cannot guess.
    async fn tenant_bytes_estimate(
        &self,
        tenant: &TenantId,
        entities: u64,
    ) -> Result<TenantBytes, NgsiError> {
        let _ = (tenant, entities);
        Err(NgsiError::OperationNotSupported(
            "tenant size estimate".into(),
        ))
    }
    /// Remove every current-state document of one tenant; `false` when the
    /// tenant did not exist. The default Tenant is emptied but keeps
    /// existing.
    async fn purge_tenant(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        let _ = tenant;
        Err(NgsiError::OperationNotSupported("tenant purge".into()))
    }
    /// 5.13 @context documents. `tenant` is the Tenant the call acts for, and
    /// `None` means no Tenant at all — the boot warm and the `Cached`
    /// write-through, which touch documents downloaded from public URLs.
    ///
    /// A backend enforces ADR-0021 on it: a `Cached` row belongs to no Tenant
    /// and is reachable with any `tenant`, and every other kind is reachable
    /// only by the Tenant named in its `owner` member. A backend that ignores
    /// the parameter fails the driver contract.
    async fn context_put(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        doc: Value,
    ) -> Result<(), NgsiError>;
    /// Read a stored @context document by id; `None` if absent or owned by
    /// another Tenant.
    async fn context_get(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
    ) -> Result<Option<Value>, NgsiError>;
    /// Read-modify-write of one stored @context document under its row lock:
    /// `None` = absent or owned by another Tenant (never an insert),
    /// `Some(Err)` = the closure rejected and nothing was committed. A usage
    /// bump or a refetch racing a delete must leave the row deleted, and two
    /// racing bumps must both count (5.13.3.5).
    async fn context_mutate<'a>(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError>;
    /// Delete a stored @context document; `false` if it was absent or owned
    /// by another Tenant.
    async fn context_delete(&self, tenant: Option<&TenantId>, id: &str) -> Result<bool, NgsiError>;
    /// Every stored @context document.
    /// Every stored `@context` row WITHOUT its `body` member — the url,
    /// localId, kind, owner and usage counters, and not the document.
    ///
    /// There is deliberately no read that returns every row WITH its body.
    /// A body is accepted up to `MAX_CONTEXT_BYTES` (5 MiB) and only the
    /// `Cached` rows are capped in number, so one such read materializes
    /// gigabytes — on the boot path, where it decides whether the broker
    /// starts at all. The callers that need one body ask for it by id
    /// ([`Self::context_get`]); nothing needs them all at once.
    async fn context_list_meta(&self, tenant: Option<&TenantId>) -> Result<Vec<Value>, NgsiError>;
}

/// The typed-slot lock, taken and released inside one closure call. A
/// poisoned slot is read through: the panic that poisoned it is already on
/// its way out of the driver, and the value behind it is whatever the
/// closure managed to write.
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The same, for taking the slot's value once the driver has answered.
fn into_inner<T>(m: std::sync::Mutex<T>) -> T {
    m.into_inner().unwrap_or_else(|p| p.into_inner())
}

/// Typed sugar over the boxed mutate seam — call sites keep their
/// `Result<T, E>` closures; the value crosses the object boundary in a
/// side slot.
pub trait CurrentStateDriverExt {
    /// Typed read-modify-write: `None` = absent, `Some(Err(e))` = the closure
    /// rejected and nothing was committed.
    fn mutate<T: Send, E: Send>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E> + Send,
    ) -> impl std::future::Future<Output = Result<Option<Result<T, E>>, NgsiError>> + Send;
    /// Typed batch read-modify-write (entities only); results align with `ids`.
    fn batch_mutate<E: Send>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        f: impl FnMut(&str, &mut Value) -> Result<(), E> + Send,
    ) -> impl std::future::Future<Output = Result<Vec<Option<Result<(), E>>>, NgsiError>> + Send;
}

impl<S: CurrentStateDriver + ?Sized> CurrentStateDriverExt for S {
    async fn mutate<T: Send, E: Send>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E> + Send,
    ) -> Result<Option<Result<T, E>>, NgsiError> {
        let slot = std::sync::Mutex::new(None);
        let r = self
            .mutate_boxed(
                tenant,
                kind,
                id,
                Box::new(|v| {
                    let r = f(v);
                    let flag = if r.is_ok() { Ok(()) } else { Err(()) };
                    *lock(&slot) = Some(r);
                    flag
                }),
            )
            .await?;
        Ok(r.and_then(|_| into_inner(slot)))
    }

    async fn batch_mutate<E: Send>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        mut f: impl FnMut(&str, &mut Value) -> Result<(), E> + Send,
    ) -> Result<Vec<Option<Result<(), E>>>, NgsiError> {
        // Errors land in the queue in closure-call order, which the trait
        // contract fixes to input order over present ids.
        let errs = std::sync::Mutex::new(std::collections::VecDeque::new());
        let r = self
            .batch_mutate_boxed(
                tenant,
                ids,
                Box::new(|id, v| match f(id, v) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        lock(&errs).push_back(e);
                        Err(())
                    }
                }),
            )
            .await?;
        let mut errs = into_inner(errs);
        let mut out = Vec::with_capacity(r.len());
        for slot in r {
            out.push(match slot {
                None => None,
                Some(Ok(())) => Some(Ok(())),
                // one queued error per rejected id is the trait contract; a
                // driver that reports more rejections than the closure raised
                // has broken it, and the batch fails rather than inventing an
                // error for the caller to read.
                Some(Err(())) => Some(Err(errs.pop_front().ok_or_else(|| {
                    NgsiError::InternalError(
                        "batch driver reported more rejections than were raised".into(),
                    )
                })?)),
            });
        }
        Ok(out)
    }
}

/// What a temporal event records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalOp {
    /// An Attribute the entity did not carry before (one event per instance).
    AttrCreated,
    /// A changed instance of an existing Attribute (4.5.6 append).
    AttrModified,
    /// 4.5.6: the Scope changed through the Core API — recorded as a scope
    /// instance whose observedAt copies modifiedAt.
    ScopeChanged,
}

/// One change the write path hands to the temporal seam. Events are
/// produced per attribute INSTANCE (the gate chain and a columnar writer
/// both work per instance) and drained per request, in order.
#[derive(Clone, Debug)]
pub struct TemporalEvent {
    /// What the event records.
    pub op: TemporalOp,
    /// Owning tenant.
    pub tenant: TenantId,
    /// Id of the entity whose history this instance belongs to.
    pub entity_id: String,
    /// The entity's meta shell (id, type, createdAt, modifiedAt, scope) as
    /// it stood after the write — what `temporal_append` creates on first
    /// touch.
    pub shell: Value,
    /// Expanded Attribute name, or `scope`.
    pub attr: String,
    /// The instance snapshot: value, datasetId, observedAt, instanceId…
    pub instance: Value,
}

/// [`TemporalDriver::event_list`]'s default: consecutive events of one Entity
/// become one `temporal_append`; a scope change goes through `mutate`, in
/// order. A driver that overrides `event_list` falls back to it.
pub async fn drain_events<D: TemporalDriver + ?Sized>(
    d: &D,
    evs: &[TemporalEvent],
) -> Result<(), NgsiError> {
    let mut i = 0;
    while i < evs.len() {
        let (tenant, id) = (&evs[i].tenant, evs[i].entity_id.as_str());
        let mut additions = serde_json::Map::new();
        let mut shell = &evs[i].shell;
        let mut j = i;
        while j < evs.len() && evs[j].tenant == *tenant && evs[j].entity_id == id {
            let ev = &evs[j];
            shell = &ev.shell;
            if ev.op == TemporalOp::ScopeChanged {
                // the additions before it first: they may create the doc the
                // scope instance goes on, and history keeps production order
                if !additions.is_empty() {
                    let pending = Value::Object(std::mem::take(&mut additions));
                    d.temporal_append(tenant, id, shell, &pending).await?;
                }
                if push_scope(d, tenant, id, &ev.instance).await?.is_none() {
                    // no temporal doc yet: create the shell, then record
                    let empty = Value::Object(serde_json::Map::new());
                    d.temporal_append(tenant, id, shell, &empty).await?;
                    push_scope(d, tenant, id, &ev.instance).await?;
                }
            } else {
                if let Some(arr) = additions
                    .entry(ev.attr.clone())
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                {
                    arr.push(ev.instance.clone());
                }
            }
            j += 1;
        }
        if !additions.is_empty() {
            d.temporal_append(tenant, id, shell, &Value::Object(additions))
                .await?;
        }
        i = j;
    }
    Ok(())
}

/// Appends a scope instance to the held temporal doc; `None` when there is
/// no doc.
async fn push_scope<D: TemporalDriver + ?Sized>(
    d: &D,
    tenant: &TenantId,
    id: &str,
    inst: &Value,
) -> Result<Option<Result<(), ()>>, NgsiError> {
    d.mutate(tenant, id, |doc| {
        let target = doc.as_object_mut().ok_or(())?;
        match target.get_mut("scope").and_then(Value::as_array_mut) {
            Some(arr) if arr.first().is_some_and(Value::is_object) => {
                arr.push(inst.clone());
            }
            _ => {
                target.insert("scope".into(), Value::Array(vec![inst.clone()]));
            }
        }
        Ok::<(), ()>(())
    })
    .await
}

/// A drained request folded into one append per run of consecutive events of
/// one Entity, as [`TemporalDriver::event_list`]'s default folds it: tenant,
/// id, the last shell, the additions by Attribute. `None` when an event
/// changes scope, which goes through `mutate` in order with the appends
/// (T-4085: a driver that writes several appends at once takes this).
pub fn fold_appends(evs: &[TemporalEvent]) -> Option<Vec<(&TenantId, &str, &Value, Value)>> {
    if evs.iter().any(|ev| ev.op == TemporalOp::ScopeChanged) {
        return None;
    }
    let mut out: Vec<(&TenantId, &str, &Value, Value)> = Vec::new();
    for ev in evs {
        let same = out
            .last()
            .is_some_and(|(t, id, _, _)| **t == ev.tenant && *id == ev.entity_id);
        if !same {
            out.push((
                &ev.tenant,
                &ev.entity_id,
                &ev.shell,
                Value::Object(serde_json::Map::new()),
            ));
        }
        if let Some((_, _, shell, Value::Object(additions))) = out.last_mut() {
            *shell = &ev.shell;
            if let Some(arr) = additions
                .entry(ev.attr.clone())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
            {
                arr.push(ev.instance.clone());
            }
        }
    }
    Some(out)
}

/// The member an attribute instance carries its write's provenance in from
/// the API to the temporal store (T-4665, platform ADR-N-054 2.3): `channel`,
/// `actor`, `operation`, `reason`, `requestId`. A store takes it out of the
/// instance before it keeps the instance, whoever put it there, so no read
/// ever answers it as data; a JSON-LD keyword is never an attribute member.
pub const PROVENANCE_KEY: &str = "@provenance";

/// Takes [`PROVENANCE_KEY`] out of an instance, returning what it held.
pub fn take_provenance(instance: &mut Value) -> Option<Value> {
    instance.as_object_mut()?.remove(PROVENANCE_KEY)
}

/// Takes [`PROVENANCE_KEY`] out of every instance of a temporal document:
/// what a store that keeps no provenance does before it keeps the document.
pub fn strip_provenance(doc: &mut Value) {
    for instances in doc.as_object_mut().into_iter().flat_map(|d| d.values_mut()) {
        for instance in instances.as_array_mut().into_iter().flatten() {
            take_provenance(instance);
        }
    }
}

/// The expanded name of the Property an Entity carries its own history
/// retention in (DM-97, ADR-N-046 2.3): a duration or `off`, the most
/// specific level, beside `expiresAt`, which ends the Entity itself.
pub const HISTORY_RETENTION: &str = "https://uri.etsi.org/ngsi-ld/default-context/historyRetention";

/// The longest history retention whose rows a store may keep unlogged
/// (T-5249, docs/src/storage.md "Short history"): a crash loses at most the
/// trail the sweep drops within this span anyway.
pub const SHORT_HISTORY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Which writes record history of [`SHORT_HISTORY`] or less, from the
/// Tenants' retention rules (DM-97): a store keeps those rows where a crash
/// may lose them and writes them no WAL.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShortHistory {
    /// (Tenant, expanded type) of every type rule that says something of
    /// history: `true` when it keeps [`SHORT_HISTORY`] or less.
    pub types: std::collections::HashMap<(String, String), bool>,
    /// Tenants whose `*` rule keeps [`SHORT_HISTORY`] or less.
    pub tenants: std::collections::HashSet<String>,
}

impl ShortHistory {
    /// Whether a write of an Entity of `types` in `tenant` keeps short
    /// history: some type's rule is short and none is longer, or no type has
    /// a rule and the Tenant's `*` rule is short. An Entity with its own
    /// [`HISTORY_RETENTION`] (`own`) never is: its level wins over both.
    #[must_use]
    pub fn is_short(&self, tenant: &str, types: &[String], own: bool) -> bool {
        if own || (self.types.is_empty() && self.tenants.is_empty()) {
            return false;
        }
        let mut ruled = types
            .iter()
            .filter_map(|t| self.types.get(&(tenant.to_owned(), t.clone())))
            .peekable();
        if ruled.peek().is_none() {
            return self.tenants.contains(tenant);
        }
        ruled.all(|short| *short)
    }
}

/// The Entities of one Tenant a history retention statement reaches (DM-97).
#[derive(Debug, Clone, Copy)]
pub enum HistoryScope<'a> {
    /// One Entity, by its own `historyRetention`.
    Entity(&'a str),
    /// The Entities with this type, but not `except_ids`, which carry their
    /// own retention.
    Type {
        /// The expanded type.
        type_iri: &'a str,
        /// Entities left out.
        except_ids: &'a [String],
    },
    /// The Entities with none of `except_types` and not in `except_ids`:
    /// what a Tenant's or the platform's default reaches.
    Rest {
        /// Types with a rule of their own.
        except_types: &'a [String],
        /// Entities left out.
        except_ids: &'a [String],
    },
}

/// Temporal storage: the entity history (Temporal Evolution) plus the raw
/// temporal documents the 5.6.13-5.6.16 edit paths operate on.
///
/// Internal bookkeeping (snapshot copies, delete cascades) degrades to
/// benign no-ops on a driver without temporal support; the CLIENT-facing
/// operations answer `OperationNotSupported` (422 per CIM 009
/// Table 6.3.2-1) instead.
#[async_trait::async_trait]
pub trait TemporalDriver: Send + Sync {
    /// The drain: one call carries a whole request's events, in production
    /// order. The default folds consecutive events of one entity into a
    /// single `temporal_append` (scope changes go through `mutate`, as
    /// 4.5.6 shapes them); a bulk writer overrides this and sees the batch.
    async fn event_list(&self, evs: &[TemporalEvent]) -> Result<(), NgsiError> {
        drain_events(self, evs).await
    }
    /// `false` = this deployment records no history (`NoTemporal`); the
    /// write path skips recording entirely.
    fn supported(&self) -> bool {
        true
    }
    /// Stored attribute instances of one tenant (inventory); a driver
    /// without history reports 0.
    async fn attr_instance_count(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        let _ = tenant;
        Ok(0)
    }
    /// [`Self::attr_instance_count`] for a read that answers without it when
    /// the count is slow (`GET /q/tenants/{tenant}`, T-5304): a store that
    /// scans for the count gives up after `bound` with an error. The default
    /// counts without a bound.
    async fn attr_instance_count_within(
        &self,
        tenant: &TenantId,
        bound: std::time::Duration,
    ) -> Result<u64, NgsiError> {
        let _ = bound;
        self.attr_instance_count(tenant).await
    }
    /// The bytes those attribute instances take in the store; a driver
    /// without history reports 0.
    async fn attr_instance_bytes(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        let _ = tenant;
        Ok(0)
    }
    /// [`Self::attr_instance_count`] and [`Self::attr_instance_bytes`] in
    /// one pass that gives up after `bound` (`GET /q/tenants/{tenant}`,
    /// T-6085), so a long history costs one scan, not two. The default asks
    /// the two reads.
    async fn attr_instance_usage(
        &self,
        tenant: &TenantId,
        bound: std::time::Duration,
    ) -> Result<(u64, u64), NgsiError> {
        let count = self.attr_instance_count_within(tenant, bound).await?;
        Ok((count, self.attr_instance_bytes(tenant).await?))
    }
    /// The store's statistics' guess at `(instances, bytes)` of one tenant's
    /// history, for a read whose exact pass ran out of time (T-6085); the
    /// answer says it is estimated. A driver without statistics cannot guess.
    async fn attr_instance_estimate(&self, tenant: &TenantId) -> Result<(u64, u64), NgsiError> {
        let _ = tenant;
        Err(NgsiError::OperationNotSupported(
            "this history store keeps no statistics to estimate from".into(),
        ))
    }
    /// Retention of history in one Tenant (`/q/retention`, T-3900, DM-97):
    /// delete at most `batch` attribute instances of the Entities `scope`
    /// names whose instant (`observedAt`, else `modifiedAt`, `deletedAt`,
    /// `createdAt`) is before `before`, a UTC DateTime. Returns how many were
    /// deleted, or with `dry_run` would be, deleting nothing; a driver
    /// without history has none.
    async fn retain_history(
        &self,
        tenant: &TenantId,
        scope: HistoryScope<'_>,
        before: &str,
        batch: usize,
        dry_run: bool,
    ) -> Result<u64, NgsiError> {
        let _ = (tenant, scope, before, batch, dry_run);
        Ok(0)
    }
    /// Retention by partition (T-4906): drop every stored slice of history
    /// whose rows all belong to Tenants in `floors` and lie before that
    /// Tenant's floor, a UTC DateTime no rule of the Tenant keeps rows below.
    /// A slice holding a row of any other Tenant stays. Returns one line per
    /// slice dropped; a driver that stores history in no droppable slices
    /// drops none, and the row sweep does the work.
    async fn drop_retained_history(
        &self,
        floors: &[(String, String)],
    ) -> Result<Vec<String>, NgsiError> {
        let _ = floors;
        Ok(Vec::new())
    }
    /// The audit sweep (T-6337, ADR-N-054 2.7): delete at most `batch` audit
    /// rows (provenance `audit: true`, which every other retention keeps)
    /// of each history table whose instant is before `before`, a UTC
    /// DateTime, in every Tenant. Returns how many went; a driver that keeps
    /// no provenance holds no audit rows.
    async fn retain_audit(&self, before: &str, batch: usize) -> Result<u64, NgsiError> {
        let _ = (before, batch);
        Ok(0)
    }
    /// The retention rules' [`ShortHistory`], replaced whole on each reload
    /// of the rules; a driver without unlogged storage ignores it.
    fn set_short_history(&self, short: ShortHistory) {
        let _ = short;
    }
    /// Every version of the rows of one Entity's history (T-4665, platform
    /// ADR-N-054 2.4), by expanded Attribute name: each instance with its
    /// `version` and `provenance`, a superseded one with `supersededAt`,
    /// oldest first per measurement. A driver that keeps no versions has none.
    async fn versions(
        &self,
        tenant: &TenantId,
        entity_id: &str,
    ) -> Result<serde_json::Map<String, Value>, NgsiError> {
        let _ = (tenant, entity_id);
        Ok(serde_json::Map::new())
    }
    /// Erasure (T-4665, platform ADR-N-054 2.6): every row of `attrs` of one
    /// Entity, the current version and every superseded one, observed at or
    /// before `before` (a UTC DateTime; `None`: every row); `attrs` `None`
    /// takes every Attribute, and the Entity's history itself once it holds
    /// no row. Answers how many rows went; a driver without history has none.
    async fn erase(
        &self,
        tenant: &TenantId,
        entity_id: &str,
        attrs: Option<&[String]>,
        before: Option<&str>,
    ) -> Result<u64, NgsiError> {
        let _ = (tenant, entity_id, attrs, before);
        Ok(0)
    }
    /// Remove the whole history of one tenant; nothing to do without history.
    async fn purge_tenant(&self, tenant: &TenantId) -> Result<(), NgsiError> {
        let _ = tenant;
        Ok(())
    }
    /// Readiness of the temporal backend; the default is always ready.
    async fn ping(&self) -> Result<(), NgsiError> {
        Ok(())
    }
    /// What this temporal driver runs on, for `/q/health`; the same
    /// contract as [`CurrentStateDriver::version_info`].
    fn version_info(&self) -> Value {
        Value::Object(serde_json::Map::new())
    }
    /// Drain: finish in-flight work and disconnect cleanly. A temporal
    /// driver configured as a backend of its own holds its own pool, and the
    /// shutdown path closes it here. May be called more than once — when one
    /// instance serves both seams it is closed through each of them — so an
    /// implementation makes it idempotent. The default has nothing to close.
    async fn close(&self) {}
    /// Auto-recording fast path: append instances, creating the meta shell
    /// on first touch — and only for an entity that still exists (5.6.6
    /// deletes history; an append overlapping the delete must not recreate
    /// it).
    async fn temporal_append(
        &self,
        tenant: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError>;
    /// Would `query_temporal` page EXACTLY under this `q` and range — the
    /// driver's own entity verdict equal to the evaluator's on every row,
    /// so a page it cuts is the page the caller would have cut? A driver
    /// that pages after an in-process evaluation, or not at all, answers
    /// `false`, and the caller pages the merged result itself (5.7.4.4).
    fn q_pushdown_exact(
        &self,
        _q: &antares_ql::QNode,
        _range: Option<&filter::InstanceRange<'_>>,
        _expand: &dyn Fn(&str) -> String,
    ) -> bool {
        false
    }
    /// Query Temporal Evolution (5.7.4) with pushdown where possible.
    async fn query_temporal(
        &self,
        tenant: &TenantId,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError>;
    /// Retrieve Temporal Evolution (5.7.3) with instance pruning.
    async fn get_temporal(
        &self,
        tenant: &TenantId,
        id: &str,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError>;
    /// Raw temporal document access (the 5.6.11-5.6.16 edit/delete paths
    /// and internal copies).
    async fn get(&self, tenant: &TenantId, id: &str) -> Result<Option<Value>, NgsiError>;
    /// Insert a temporal document; `false` if the id already exists.
    async fn create(&self, tenant: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError>;
    /// Insert or replace a temporal document. `true` means it was ALREADY
    /// there and this call replaced it; `false` means this call created it,
    /// the same polarity as the current-state seam.
    async fn upsert(&self, tenant: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError>;
    /// Delete an entity's whole history; `false` if it had none.
    async fn delete(&self, tenant: &TenantId, id: &str) -> Result<bool, NgsiError>;
    /// Every temporal document in the tenant.
    async fn list(&self, tenant: &TenantId) -> Result<Vec<Value>, NgsiError>;
    /// Read-modify-write of one temporal document under its row lock;
    /// `None` = absent (never an insert), `Some(Err)` = rejected, not committed.
    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError>;
}

/// Typed mutate sugar for the temporal seam, same slot trick as
/// [`CurrentStateDriverExt`].
pub trait TemporalDriverExt {
    /// Typed read-modify-write of one temporal document: `None` = absent,
    /// `Some(Err(e))` = the closure rejected and nothing was committed.
    fn mutate<T: Send, E: Send>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E> + Send,
    ) -> impl std::future::Future<Output = Result<Option<Result<T, E>>, NgsiError>> + Send;
}

impl<S: TemporalDriver + ?Sized> TemporalDriverExt for S {
    async fn mutate<T: Send, E: Send>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E> + Send,
    ) -> Result<Option<Result<T, E>>, NgsiError> {
        let slot = std::sync::Mutex::new(None);
        let r = self
            .mutate_boxed(
                tenant,
                id,
                Box::new(|v| {
                    let r = f(v);
                    let flag = if r.is_ok() { Ok(()) } else { Err(()) };
                    *lock(&slot) = Some(r);
                    flag
                }),
            )
            .await?;
        Ok(r.and_then(|_| into_inner(slot)))
    }
}

/// The no-history driver: temporal OFF as a driver choice. Client-facing
/// reads answer `OperationNotSupported`; the recorder and the internal
/// bookkeeping paths degrade to no-ops.
pub struct NoTemporal;

fn unsupported() -> NgsiError {
    NgsiError::OperationNotSupported("no temporal store is configured".into())
}

#[async_trait::async_trait]
impl TemporalDriver for NoTemporal {
    fn supported(&self) -> bool {
        false
    }
    async fn temporal_append(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _shell: &Value,
        _additions: &Value,
    ) -> Result<(), NgsiError> {
        Ok(())
    }
    async fn query_temporal(
        &self,
        _tenant: &TenantId,
        _f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError> {
        Err(unsupported())
    }
    async fn get_temporal(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        Err(unsupported())
    }
    async fn get(&self, _tenant: &TenantId, _id: &str) -> Result<Option<Value>, NgsiError> {
        Ok(None)
    }
    async fn create(&self, _tenant: &TenantId, _id: &str, _doc: Value) -> Result<bool, NgsiError> {
        Ok(false)
    }
    async fn upsert(&self, _tenant: &TenantId, _id: &str, _doc: Value) -> Result<bool, NgsiError> {
        Ok(false)
    }
    async fn delete(&self, _tenant: &TenantId, _id: &str) -> Result<bool, NgsiError> {
        Ok(false)
    }
    async fn list(&self, _tenant: &TenantId) -> Result<Vec<Value>, NgsiError> {
        Ok(Vec::new())
    }
    async fn mutate_boxed<'a>(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-5249: a short type rule wins over the Tenant's level, a longer type
    /// rule among the types or the Entity's own retention keeps it logged.
    #[test]
    fn short_history_takes_the_most_specific_level() {
        let mut s = ShortHistory::default();
        let ty = |t: &str| vec![t.to_owned()];
        assert!(!s.is_short("city", &ty("Bus"), false), "no rule, not short");
        s.types.insert(("city".into(), "Bus".into()), true);
        s.types.insert(("city".into(), "Stop".into()), false);
        assert!(s.is_short("city", &ty("Bus"), false));
        assert!(!s.is_short("city", &ty("Bus"), true), "own retention wins");
        assert!(!s.is_short("town", &ty("Bus"), false), "another Tenant");
        let both = vec!["Bus".to_owned(), "Stop".to_owned()];
        assert!(!s.is_short("city", &both, false), "a longer type keeps it");
        assert!(!s.is_short("city", &ty("Tram"), false), "no `*` rule");
        s.tenants.insert("city".into());
        assert!(s.is_short("city", &ty("Tram"), false), "the `*` rule");
        assert!(!s.is_short("city", &ty("Stop"), false), "a type rule wins");
    }

    /// The mode list is one source of truth: every variant round-trips
    /// through its name, and the message for an unknown name lists all of
    /// them — a backend added to the enum cannot be missing from either.
    #[test]
    fn every_store_mode_round_trips_and_the_error_lists_them_all() {
        for m in StoreMode::ALL {
            let back = m
                .as_str()
                .parse::<StoreMode>()
                .unwrap_or_else(|e| panic!("{m} must parse back: {e}"));
            assert_eq!(back, m);
        }
        let err = "mongo".parse::<StoreMode>().expect_err("unknown mode");
        assert!(err.contains("mongo"), "{err}");
        for m in StoreMode::ALL {
            assert!(err.contains(m.as_str()), "the message must name {m}: {err}");
        }
    }

    /// A minimal driver proving the boxed seam round-trips typed results:
    /// present row → the closure's T and E cross intact; absent → None.
    struct OneDoc(std::sync::Mutex<Option<Value>>);
    #[async_trait::async_trait]
    impl CurrentStateDriver for OneDoc {
        async fn ping(&self) -> Result<(), NgsiError> {
            Ok(())
        }
        async fn close(&self) {}
        fn set_change_hook(&self, _h: ChangeHook) {}
        async fn create(
            &self,
            _t: &TenantId,
            _k: Kind,
            _id: &str,
            doc: Value,
        ) -> Result<bool, NgsiError> {
            *self.0.lock().expect("lock") = Some(doc);
            Ok(true)
        }
        async fn batch_create(
            &self,
            _t: &TenantId,
            _items: Vec<(String, Value)>,
        ) -> Result<Vec<bool>, NgsiError> {
            unimplemented!()
        }
        async fn list_page(
            &self,
            _t: &TenantId,
            _k: Kind,
            _after: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<Value>, NgsiError> {
            unimplemented!()
        }
        async fn list_slice(
            &self,
            _t: &TenantId,
            _k: Kind,
            _offset: usize,
            _limit: usize,
        ) -> Result<(Vec<Value>, usize), NgsiError> {
            unimplemented!()
        }
        async fn batch_delete(
            &self,
            _t: &TenantId,
            _ids: &[String],
        ) -> Result<Vec<bool>, NgsiError> {
            unimplemented!()
        }
        async fn batch_upsert(
            &self,
            _t: &TenantId,
            _items: Vec<(String, Value)>,
        ) -> Result<Vec<bool>, NgsiError> {
            unimplemented!()
        }
        async fn upsert(
            &self,
            _t: &TenantId,
            _k: Kind,
            _id: &str,
            _doc: Value,
        ) -> Result<bool, NgsiError> {
            unimplemented!()
        }
        async fn get(
            &self,
            _t: &TenantId,
            _k: Kind,
            _id: &str,
        ) -> Result<Option<Value>, NgsiError> {
            Ok(self.0.lock().expect("lock").clone())
        }
        async fn delete(&self, _t: &TenantId, _k: Kind, _id: &str) -> Result<bool, NgsiError> {
            unimplemented!()
        }
        async fn delete_entity_if(
            &self,
            _t: &TenantId,
            _id: &str,
            _keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
        ) -> Result<bool, NgsiError> {
            unimplemented!()
        }
        async fn list(&self, _t: &TenantId, _k: Kind) -> Result<Vec<Value>, NgsiError> {
            unimplemented!()
        }
        async fn matching_registrations(
            &self,
            _t: &TenantId,
            _ids: Option<&[String]>,
            _types: Option<&[String]>,
        ) -> Result<Vec<Value>, NgsiError> {
            unimplemented!()
        }
        async fn query_entities(
            &self,
            _t: &TenantId,
            _f: &filter::EntityFilter<'_>,
        ) -> Result<filter::QueryOutcome, NgsiError> {
            unimplemented!()
        }
        async fn mutate_boxed<'a>(
            &self,
            _t: &TenantId,
            _k: Kind,
            _id: &str,
            f: MutateFn<'a>,
        ) -> Result<Option<Result<(), ()>>, NgsiError> {
            let mut guard = self.0.lock().expect("lock");
            match guard.as_mut() {
                None => Ok(None),
                Some(v) => {
                    let mut copy = v.clone();
                    match f(&mut copy) {
                        Ok(()) => {
                            *v = copy;
                            Ok(Some(Ok(())))
                        }
                        Err(()) => Ok(Some(Err(()))),
                    }
                }
            }
        }
        async fn batch_mutate_boxed<'a>(
            &self,
            _t: &TenantId,
            _ids: &[String],
            _f: BatchMutateFn<'a>,
        ) -> Result<Vec<Option<Result<(), ()>>>, NgsiError> {
            unimplemented!()
        }
        async fn tenant_exists(&self, _t: &TenantId) -> Result<bool, NgsiError> {
            Ok(true)
        }
        async fn subscription_tenants(&self) -> Result<Vec<String>, NgsiError> {
            Ok(Vec::new())
        }
        async fn context_put(
            &self,
            _t: Option<&TenantId>,
            _id: &str,
            _doc: Value,
        ) -> Result<(), NgsiError> {
            Ok(())
        }
        async fn context_get(
            &self,
            _t: Option<&TenantId>,
            _id: &str,
        ) -> Result<Option<Value>, NgsiError> {
            Ok(None)
        }
        async fn context_mutate<'a>(
            &self,
            _t: Option<&TenantId>,
            _id: &str,
            _f: MutateFn<'a>,
        ) -> Result<Option<Result<(), ()>>, NgsiError> {
            Ok(None)
        }
        async fn context_delete(
            &self,
            _t: Option<&TenantId>,
            _id: &str,
        ) -> Result<bool, NgsiError> {
            Ok(false)
        }
        async fn context_list_meta(&self, _t: Option<&TenantId>) -> Result<Vec<Value>, NgsiError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn typed_mutate_round_trips_through_the_boxed_seam() {
        let t = TenantId::new("t").expect("tenant");
        let d: std::sync::Arc<dyn CurrentStateDriver> =
            std::sync::Arc::new(OneDoc(std::sync::Mutex::new(None)));
        // absent → None, closure never runs
        let r = d
            .mutate::<u32, &str>(&t, Kind::Entity, "x", |_| panic!("must not run"))
            .await
            .expect("driver ok");
        assert!(r.is_none());
        d.create(&t, Kind::Entity, "x", serde_json::json!({"n": 1}))
            .await
            .expect("create");
        // typed success crosses the boundary AND the write commits
        let r = d
            .mutate::<u32, &str>(&t, Kind::Entity, "x", |v| {
                v["n"] = serde_json::json!(2);
                Ok(7)
            })
            .await
            .expect("driver ok");
        assert_eq!(r, Some(Ok(7)));
        assert_eq!(
            d.get(&t, Kind::Entity, "x")
                .await
                .expect("get")
                .expect("doc")["n"],
            2
        );
        // typed error crosses the boundary AND the write is discarded
        let r = d
            .mutate::<u32, &str>(&t, Kind::Entity, "x", |v| {
                v["n"] = serde_json::json!(99);
                Err("rejected")
            })
            .await
            .expect("driver ok");
        assert_eq!(r, Some(Err("rejected")));
        assert_eq!(
            d.get(&t, Kind::Entity, "x")
                .await
                .expect("get")
                .expect("doc")["n"],
            2,
            "a rejecting closure must not commit"
        );
    }

    /// A temporal driver that only records what the seam hands it: appends
    /// as (id, additions) in call order, scope mutations on a held doc.
    #[derive(Default)]
    struct Recorder {
        appends: std::sync::Mutex<Vec<(String, Value)>>,
        doc: std::sync::Mutex<Option<Value>>,
    }
    #[async_trait::async_trait]
    impl TemporalDriver for Recorder {
        async fn temporal_append(
            &self,
            _t: &TenantId,
            id: &str,
            shell: &Value,
            additions: &Value,
        ) -> Result<(), NgsiError> {
            self.appends
                .lock()
                .expect("lock")
                .push((id.to_owned(), additions.clone()));
            // a real driver creates the doc from the shell on first touch
            self.doc
                .lock()
                .expect("lock")
                .get_or_insert_with(|| shell.clone());
            Ok(())
        }
        async fn query_temporal(
            &self,
            _t: &TenantId,
            _f: &filter::TemporalFilter<'_>,
        ) -> Result<filter::TemporalOutcome, NgsiError> {
            unimplemented!()
        }
        async fn get_temporal(
            &self,
            _t: &TenantId,
            _id: &str,
            _f: &filter::TemporalFilter<'_>,
        ) -> Result<Option<Value>, NgsiError> {
            unimplemented!()
        }
        async fn get(&self, _t: &TenantId, _id: &str) -> Result<Option<Value>, NgsiError> {
            Ok(self.doc.lock().expect("lock").clone())
        }
        async fn create(&self, _t: &TenantId, _id: &str, doc: Value) -> Result<bool, NgsiError> {
            *self.doc.lock().expect("lock") = Some(doc);
            Ok(true)
        }
        async fn upsert(&self, _t: &TenantId, _id: &str, _doc: Value) -> Result<bool, NgsiError> {
            unimplemented!()
        }
        async fn delete(&self, _t: &TenantId, _id: &str) -> Result<bool, NgsiError> {
            unimplemented!()
        }
        async fn list(&self, _t: &TenantId) -> Result<Vec<Value>, NgsiError> {
            unimplemented!()
        }
        async fn mutate_boxed<'a>(
            &self,
            _t: &TenantId,
            _id: &str,
            f: MutateFn<'a>,
        ) -> Result<Option<Result<(), ()>>, NgsiError> {
            let mut guard = self.doc.lock().expect("lock");
            match guard.as_mut() {
                None => Ok(None),
                Some(v) => Ok(Some(f(v))),
            }
        }
    }

    fn ev(op: TemporalOp, id: &str, attr: &str, n: u32) -> TemporalEvent {
        TemporalEvent {
            op,
            tenant: TenantId::new("t").expect("tenant"),
            entity_id: id.into(),
            shell: serde_json::json!({"id": id, "type": ["T"]}),
            attr: attr.into(),
            instance: serde_json::json!({"type": "Property", "value": n}),
        }
    }

    /// The drain folds one request's events into ONE append per entity run
    /// — a 2-attribute entity is one call carrying both, not two — and
    /// keeps production order across entities.
    #[test]
    fn fold_appends_is_one_append_per_entity_run_and_none_for_a_scope_change() {
        let evs = [
            ev(TemporalOp::AttrCreated, "urn:a", "speed", 1),
            ev(TemporalOp::AttrCreated, "urn:a", "speed", 2),
            ev(TemporalOp::AttrCreated, "urn:b", "speed", 3),
            ev(TemporalOp::AttrCreated, "urn:a", "load", 4),
        ];
        let folded = fold_appends(&evs).expect("no scope change");
        let ids: Vec<&str> = folded.iter().map(|(_, id, _, _)| *id).collect();
        assert_eq!(ids, ["urn:a", "urn:b", "urn:a"]);
        assert_eq!(folded[0].3["speed"].as_array().map(Vec::len), Some(2));
        assert!(fold_appends(&[]).is_some_and(|f| f.is_empty()));
        let scope = ev(TemporalOp::ScopeChanged, "urn:a", "scope", 5);
        assert!(fold_appends(&[evs[0].clone(), scope]).is_none());
    }

    #[tokio::test]
    async fn event_list_folds_a_request_into_one_append_per_entity_run() {
        let d = Recorder::default();
        d.event_list(&[
            ev(TemporalOp::AttrCreated, "urn:a", "speed", 1),
            ev(TemporalOp::AttrCreated, "urn:a", "speed", 2),
            ev(TemporalOp::AttrModified, "urn:a", "heading", 3),
            ev(TemporalOp::AttrModified, "urn:b", "speed", 4),
            ev(TemporalOp::AttrModified, "urn:a", "speed", 5),
        ])
        .await
        .expect("drain ok");
        let appends = d.appends.lock().expect("lock").clone();
        let ids: Vec<&str> = appends.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["urn:a", "urn:b", "urn:a"],
            "one append per entity run, in order"
        );
        let first = &appends[0].1;
        assert_eq!(first["speed"].as_array().map(Vec::len), Some(2), "{first}");
        assert_eq!(
            first["heading"].as_array().map(Vec::len),
            Some(1),
            "{first}"
        );
        assert!(
            first.get("value").is_none(),
            "instances live under their attribute: {first}"
        );
        assert_eq!(appends[2].1["speed"][0]["value"], 5);
    }

    /// 4.5.6: a scope change becomes a scope instance on the held temporal
    /// doc (array-of-instances form), not an attribute append.
    #[tokio::test]
    async fn event_list_records_scope_changes_as_scope_instances() {
        let t = TenantId::new("t").expect("tenant");
        let d = Recorder::default();
        d.create(
            &t,
            "urn:a",
            serde_json::json!({"id": "urn:a", "scope": "/old"}),
        )
        .await
        .expect("create");
        let mut scope = ev(TemporalOp::ScopeChanged, "urn:a", "scope", 0);
        scope.instance = serde_json::json!({"type": "Property", "value": "/new",
                                            "observedAt": "2026-01-01T00:00:00Z"});
        d.event_list(&[scope]).await.expect("drain ok");
        assert!(
            d.appends.lock().expect("lock").is_empty(),
            "no attribute append for a scope change"
        );
        let doc = d.get(&t, "urn:a").await.expect("get").expect("doc");
        assert_eq!(doc["scope"][0]["value"], "/new", "{doc}");
        assert_eq!(
            doc["scope"].as_array().map(Vec::len),
            Some(1),
            "the plain scope became an instance array: {doc}"
        );
    }

    /// F-ANT1-015: a scope change first in a run, with no temporal doc yet,
    /// still lands: the shell is created rather than the instance dropped,
    /// and additions queued before it are appended first.
    #[tokio::test]
    async fn event_list_keeps_a_scope_change_on_an_entity_without_history() {
        let t = TenantId::new("t").expect("tenant");
        let d = Recorder::default();
        d.event_list(&[
            ev(TemporalOp::ScopeChanged, "urn:a", "scope", 7),
            ev(TemporalOp::AttrCreated, "urn:a", "speed", 1),
        ])
        .await
        .expect("drain ok");
        let doc = d.get(&t, "urn:a").await.expect("get").expect("doc");
        assert_eq!(doc["scope"][0]["value"], 7, "{doc}");

        let d = Recorder::default();
        d.event_list(&[
            ev(TemporalOp::AttrCreated, "urn:a", "speed", 1),
            ev(TemporalOp::ScopeChanged, "urn:a", "scope", 7),
        ])
        .await
        .expect("drain ok");
        let appends = d.appends.lock().expect("lock").clone();
        assert_eq!(appends.len(), 1, "{appends:?}");
        assert_eq!(appends[0].1["speed"][0]["value"], 1);
        let doc = d.get(&t, "urn:a").await.expect("get").expect("doc");
        assert_eq!(doc["scope"][0]["value"], 7, "{doc}");
    }

    #[tokio::test]
    async fn no_temporal_degrades_without_panicking() {
        let t = TenantId::new("t").expect("tenant");
        let d: std::sync::Arc<dyn TemporalDriver> = std::sync::Arc::new(NoTemporal);
        assert!(!d.supported());
        // client-facing reads: the spec error, not a panic
        let e = match d
            .query_temporal(&t, &filter::TemporalFilter::default())
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("query_temporal on NoTemporal must be unsupported"),
        };
        assert_eq!(e.status(), 422);
        assert_eq!(e.kind(), "OperationNotSupported");
        // internal bookkeeping: benign no-ops
        assert!(d
            .temporal_append(&t, "x", &Value::Null, &Value::Null)
            .await
            .is_ok());
        assert!(!d.delete(&t, "x").await.expect("ok"));
        assert!(d.list(&t).await.expect("ok").is_empty());
        assert!(d
            .mutate::<(), ()>(&t, "x", |_| Ok(()))
            .await
            .expect("ok")
            .is_none());
    }
}
