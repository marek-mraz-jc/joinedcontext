// SPDX-License-Identifier: EUPL-1.2
//! A delegating current-state driver that fails the calls a test names.
//! Every method of the trait delegates to the real store, so the data a
//! refusal guards is the real store's; a test arms [`FaultStore::fail`]
//! with a rule over (method, tenant, kind) and every matching call answers
//! `InternalError` until the rule is replaced. Included by `#[path]` from the
//! crates whose store-failure arms it reaches.

use antares_model::{NgsiError, TenantId};
use antares_sql::store::{any::AnyStore, Store};
use antares_store::{
    confirmations, filter, BatchMutateFn, ChangeHook, ContextCaps, ContextPut, CurrentStateDriver,
    Delivery, Kind, MutateFn, TenantBytes, TenantStanding, TenantStats,
};
use serde_json::Value;
use std::sync::{Arc, Mutex, PoisonError};

/// The rule a [`FaultStore`] fails calls by: method name, tenant, kind.
pub type Rule = Box<dyn Fn(&str, Option<&str>, Option<Kind>) -> bool + Send + Sync>;

/// The driver: the real store behind a rule.
pub struct FaultStore {
    inner: Arc<dyn CurrentStateDriver>,
    rule: Mutex<Option<Rule>>,
}

impl FaultStore {
    /// A broker state over the memory store behind a `FaultStore` with no
    /// call failing yet, and the handle that arms it.
    #[must_use]
    pub fn state(host: &str) -> (antares_api::AppState, Arc<Self>) {
        let store = Arc::new(AnyStore::Mem(Store::default()));
        let fault = Arc::new(Self {
            inner: store.clone(),
            rule: Mutex::new(None),
        });
        let state =
            antares_api::AppState::with_drivers(host.into(), fault.clone(), store, "memory");
        (state, fault)
    }

    /// From now on every call `rule` matches fails.
    pub fn fail(
        &self,
        rule: impl Fn(&str, Option<&str>, Option<Kind>) -> bool + Send + Sync + 'static,
    ) {
        *self.rule.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(rule));
    }

    fn trip(
        &self,
        method: &str,
        tenant: Option<&str>,
        kind: Option<Kind>,
    ) -> Result<(), NgsiError> {
        let rule = self.rule.lock().unwrap_or_else(PoisonError::into_inner);
        match rule.as_ref() {
            Some(r) if r(method, tenant, kind) => Err(NgsiError::InternalError(format!(
                "injected: {method} failed"
            ))),
            _ => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl CurrentStateDriver for FaultStore {
    async fn ping(&self) -> Result<(), NgsiError> {
        self.trip("ping", None, None)?;
        self.inner.ping().await
    }

    async fn wedged(&self) -> bool {
        self.inner.wedged().await
    }

    fn commit_queue(&self) -> Option<(usize, usize)> {
        self.inner.commit_queue()
    }

    async fn close(&self) {
        self.inner.close().await;
    }

    fn version_info(&self) -> Value {
        self.inner.version_info()
    }

    fn set_change_hook(&self, h: ChangeHook) {
        self.inner.set_change_hook(h);
    }

    fn set_outbox(&self, on: bool) {
        self.inner.set_outbox(on);
    }

    async fn outbox_peek(&self, limit: i64) -> Result<Vec<(i64, String, Value)>, NgsiError> {
        self.trip("outbox_peek", None, None)?;
        self.inner.outbox_peek(limit).await
    }

    async fn outbox_ack(&self, seqs: &[i64]) -> Result<u64, NgsiError> {
        self.trip("outbox_ack", None, None)?;
        self.inner.outbox_ack(seqs).await
    }

    async fn outbox_retain(&self, tenant: &TenantId, seqs: &[i64]) -> Result<u64, NgsiError> {
        self.trip("outbox_retain", Some(tenant.as_str()), None)?;
        self.inner.outbox_retain(tenant, seqs).await
    }

    async fn outbox_event(&self, seq: i64, tenant: &TenantId) -> Result<Option<Value>, NgsiError> {
        self.trip("outbox_event", Some(tenant.as_str()), None)?;
        self.inner.outbox_event(seq, tenant).await
    }

    async fn create(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        self.trip("create", Some(tenant.as_str()), Some(kind))?;
        self.inner.create(tenant, kind, id, doc).await
    }

    async fn batch_create(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        self.trip("batch_create", Some(tenant.as_str()), None)?;
        self.inner.batch_create(tenant, items).await
    }

    async fn batch_delete(
        &self,
        tenant: &TenantId,
        ids: &[String],
    ) -> Result<Vec<bool>, NgsiError> {
        self.trip("batch_delete", Some(tenant.as_str()), None)?;
        self.inner.batch_delete(tenant, ids).await
    }

    async fn batch_upsert(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        self.trip("batch_upsert", Some(tenant.as_str()), None)?;
        self.inner.batch_upsert(tenant, items).await
    }

    async fn upsert(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        self.trip("upsert", Some(tenant.as_str()), Some(kind))?;
        self.inner.upsert(tenant, kind, id, doc).await
    }

    async fn get(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        self.trip("get", Some(tenant.as_str()), Some(kind))?;
        self.inner.get(tenant, kind, id).await
    }

    async fn get_entities(
        &self,
        tenant: &TenantId,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Value>, NgsiError> {
        self.trip("get_entities", Some(tenant.as_str()), None)?;
        self.inner.get_entities(tenant, ids).await
    }

    async fn delete(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<bool, NgsiError> {
        self.trip("delete", Some(tenant.as_str()), Some(kind))?;
        self.inner.delete(tenant, kind, id).await
    }

    async fn delete_entity_if(
        &self,
        tenant: &TenantId,
        id: &str,
        keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
    ) -> Result<bool, NgsiError> {
        self.trip("delete_entity_if", Some(tenant.as_str()), None)?;
        self.inner.delete_entity_if(tenant, id, keep).await
    }

    async fn list(&self, tenant: &TenantId, kind: Kind) -> Result<Vec<Value>, NgsiError> {
        self.trip("list", Some(tenant.as_str()), Some(kind))?;
        self.inner.list(tenant, kind).await
    }

    async fn list_digests(
        &self,
        tenant: &TenantId,
        kind: Kind,
    ) -> Result<Vec<(String, u64)>, NgsiError> {
        self.trip("list_digests", Some(tenant.as_str()), Some(kind))?;
        self.inner.list_digests(tenant, kind).await
    }

    async fn doc_revision(&self, tenant: &TenantId, kind: Kind) -> Result<Option<u64>, NgsiError> {
        self.trip("doc_revision", Some(tenant.as_str()), Some(kind))?;
        self.inner.doc_revision(tenant, kind).await
    }

    async fn list_page(
        &self,
        tenant: &TenantId,
        kind: Kind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, NgsiError> {
        self.trip("list_page", Some(tenant.as_str()), Some(kind))?;
        self.inner.list_page(tenant, kind, after, limit).await
    }

    async fn list_slice(
        &self,
        tenant: &TenantId,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Value>, usize), NgsiError> {
        self.trip("list_slice", Some(tenant.as_str()), Some(kind))?;
        self.inner.list_slice(tenant, kind, offset, limit).await
    }

    async fn matching_registrations(
        &self,
        tenant: &TenantId,
        ids: Option<&[String]>,
        types: Option<&[String]>,
    ) -> Result<Vec<Value>, NgsiError> {
        self.trip("matching_registrations", Some(tenant.as_str()), None)?;
        self.inner.matching_registrations(tenant, ids, types).await
    }

    async fn query_entities(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome, NgsiError> {
        self.trip("query_entities", Some(tenant.as_str()), None)?;
        self.inner.query_entities(tenant, f).await
    }

    async fn query_candidates(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome, NgsiError> {
        self.trip("query_candidates", Some(tenant.as_str()), None)?;
        self.inner.query_candidates(tenant, f).await
    }

    async fn query_entities_text(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome<Box<serde_json::value::RawValue>>, NgsiError> {
        self.trip("query_entities_text", Some(tenant.as_str()), None)?;
        self.inner.query_entities_text(tenant, f).await
    }

    async fn entity_type_names(&self, tenant: &TenantId) -> Result<Option<Vec<String>>, NgsiError> {
        self.trip("entity_type_names", Some(tenant.as_str()), None)?;
        self.inner.entity_type_names(tenant).await
    }

    async fn entity_types_beside(
        &self,
        tenant: &TenantId,
        any_of: &[String],
    ) -> Result<Option<Vec<String>>, NgsiError> {
        self.trip("entity_types_beside", Some(tenant.as_str()), None)?;
        self.inner.entity_types_beside(tenant, any_of).await
    }

    async fn remapped_id(&self, tenant: &TenantId, id: &str) -> Result<Option<String>, NgsiError> {
        self.trip("remapped_id", Some(tenant.as_str()), None)?;
        self.inner.remapped_id(tenant, id).await
    }

    async fn entity_id_segments(
        &self,
        tenant: &TenantId,
        lead: &str,
        sep: char,
        max: usize,
    ) -> Result<Option<Vec<String>>, NgsiError> {
        self.trip("entity_id_segments", Some(tenant.as_str()), None)?;
        self.inner.entity_id_segments(tenant, lead, sep, max).await
    }

    async fn entity_attr_pairs(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<Vec<(String, String)>>, NgsiError> {
        self.trip("entity_attr_pairs", Some(tenant.as_str()), None)?;
        self.inner.entity_attr_pairs(tenant).await
    }

    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.trip("mutate_boxed", Some(tenant.as_str()), Some(kind))?;
        self.inner.mutate_boxed(tenant, kind, id, f).await
    }

    async fn batch_mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        f: BatchMutateFn<'a>,
    ) -> Result<Vec<Option<Result<(), ()>>>, NgsiError> {
        self.trip("batch_mutate_boxed", Some(tenant.as_str()), None)?;
        self.inner.batch_mutate_boxed(tenant, ids, f).await
    }

    async fn record_delivery(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        now: &str,
    ) -> Result<Option<Delivery>, NgsiError> {
        self.trip("record_delivery", Some(tenant.as_str()), Some(kind))?;
        self.inner.record_delivery(tenant, kind, id, now).await
    }

    async fn record_failure(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        prev_success: Option<Value>,
        now: &str,
    ) -> Result<Option<Value>, NgsiError> {
        self.trip("record_failure", Some(tenant.as_str()), Some(kind))?;
        self.inner
            .record_failure(tenant, kind, id, prev_success, now)
            .await
    }

    async fn record_deliveries(
        &self,
        tenant: &TenantId,
        kind: Kind,
        attempts: &[(&str, &str, i64)],
    ) -> Result<Vec<Option<Delivery>>, NgsiError> {
        self.trip("record_deliveries", Some(tenant.as_str()), Some(kind))?;
        self.inner.record_deliveries(tenant, kind, attempts).await
    }

    async fn confirmation_ask(
        &self,
        tenant: &TenantId,
        id: &str,
        question: &confirmations::Confirmation,
        caps: confirmations::Caps,
    ) -> Result<bool, NgsiError> {
        self.trip("confirmation_ask", Some(tenant.as_str()), None)?;
        self.inner
            .confirmation_ask(tenant, id, question, caps)
            .await
    }

    async fn confirmation_find(
        &self,
        tenant: &TenantId,
        id: &str,
    ) -> Result<Option<confirmations::Confirmation>, NgsiError> {
        self.trip("confirmation_find", Some(tenant.as_str()), None)?;
        self.inner.confirmation_find(tenant, id).await
    }

    async fn confirmation_decide(
        &self,
        tenant: &TenantId,
        id: &str,
        accept: bool,
    ) -> Result<Option<confirmations::Confirmation>, NgsiError> {
        self.trip("confirmation_decide", Some(tenant.as_str()), None)?;
        self.inner.confirmation_decide(tenant, id, accept).await
    }

    async fn confirmation_take(
        &self,
        tenant: &TenantId,
        id: &str,
        subject: &confirmations::Subject,
    ) -> Result<confirmations::Taken, NgsiError> {
        self.trip("confirmation_take", Some(tenant.as_str()), None)?;
        self.inner.confirmation_take(tenant, id, subject).await
    }

    async fn confirmation_spend(
        &self,
        tenant: &TenantId,
        id: &str,
        subject: &confirmations::Subject,
        ttl: std::time::Duration,
        caps: confirmations::Caps,
    ) -> Result<bool, NgsiError> {
        self.trip("confirmation_spend", Some(tenant.as_str()), None)?;
        self.inner
            .confirmation_spend(tenant, id, subject, ttl, caps)
            .await
    }

    async fn confirmation_sweep(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        self.trip("confirmation_sweep", Some(tenant.as_str()), None)?;
        self.inner.confirmation_sweep(tenant).await
    }

    async fn sweep_expired(&self) -> usize {
        self.inner.sweep_expired().await
    }

    async fn retain_current(
        &self,
        tenant: &TenantId,
        type_iri: &str,
        before: &str,
        batch: usize,
        dry_run: bool,
    ) -> Result<u64, NgsiError> {
        self.trip("retain_current", Some(tenant.as_str()), None)?;
        self.inner
            .retain_current(tenant, type_iri, before, batch, dry_run)
            .await
    }

    async fn history_overrides(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, Value)>, NgsiError> {
        self.trip("history_overrides", None, None)?;
        self.inner.history_overrides(limit).await
    }

    async fn record_forward(
        &self,
        tenant: &TenantId,
        id: &str,
        now: &str,
        ok: bool,
    ) -> Result<Option<Value>, NgsiError> {
        self.trip("record_forward", Some(tenant.as_str()), None)?;
        self.inner.record_forward(tenant, id, now, ok).await
    }

    async fn tenant_exists(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        self.trip("tenant_exists", Some(tenant.as_str()), None)?;
        self.inner.tenant_exists(tenant).await
    }

    async fn tenant_standing(&self, tenant: &TenantId) -> Result<TenantStanding, NgsiError> {
        self.trip("tenant_standing", Some(tenant.as_str()), None)?;
        self.inner.tenant_standing(tenant).await
    }

    async fn subscription_tenants(&self) -> Result<Vec<String>, NgsiError> {
        self.trip("subscription_tenants", None, None)?;
        self.inner.subscription_tenants().await
    }

    async fn tenant_ids(&self) -> Result<Vec<String>, NgsiError> {
        self.trip("tenant_ids", None, None)?;
        self.inner.tenant_ids().await
    }

    async fn tenant_stats_one(&self, tenant: &TenantId) -> Result<Option<TenantStats>, NgsiError> {
        self.trip("tenant_stats_one", Some(tenant.as_str()), None)?;
        self.inner.tenant_stats_one(tenant).await
    }

    async fn tenant_bytes(&self, tenant: &TenantId) -> Result<TenantBytes, NgsiError> {
        self.trip("tenant_bytes", Some(tenant.as_str()), None)?;
        self.inner.tenant_bytes(tenant).await
    }

    async fn tenant_bytes_estimate(
        &self,
        tenant: &TenantId,
        entities: u64,
    ) -> Result<TenantBytes, NgsiError> {
        self.trip("tenant_bytes_estimate", Some(tenant.as_str()), None)?;
        self.inner.tenant_bytes_estimate(tenant, entities).await
    }

    async fn purge_tenant(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        self.trip("purge_tenant", Some(tenant.as_str()), None)?;
        self.inner.purge_tenant(tenant).await
    }

    async fn context_put(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        doc: Value,
    ) -> Result<(), NgsiError> {
        self.trip("context_put", tenant.map(TenantId::as_str), None)?;
        self.inner.context_put(tenant, id, doc).await
    }

    async fn context_put_capped(
        &self,
        tenant: &TenantId,
        id: &str,
        doc: Value,
        caps: ContextCaps,
    ) -> Result<ContextPut, NgsiError> {
        self.trip("context_put_capped", Some(tenant.as_str()), None)?;
        self.inner.context_put_capped(tenant, id, doc, caps).await
    }

    async fn context_get(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        self.trip("context_get", tenant.map(TenantId::as_str), None)?;
        self.inner.context_get(tenant, id).await
    }

    async fn context_mutate<'a>(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.trip("context_mutate", tenant.map(TenantId::as_str), None)?;
        self.inner.context_mutate(tenant, id, f).await
    }

    async fn context_delete(&self, tenant: Option<&TenantId>, id: &str) -> Result<bool, NgsiError> {
        self.trip("context_delete", tenant.map(TenantId::as_str), None)?;
        self.inner.context_delete(tenant, id).await
    }

    async fn context_list_meta(&self, tenant: Option<&TenantId>) -> Result<Vec<Value>, NgsiError> {
        self.trip("context_list_meta", tenant.map(TenantId::as_str), None)?;
        self.inner.context_list_meta(tenant).await
    }
}
