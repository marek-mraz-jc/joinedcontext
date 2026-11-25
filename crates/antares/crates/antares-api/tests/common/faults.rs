//! A delegating driver pair that fails chosen calls: every method of
//! [`CurrentStateDriver`] and [`TemporalDriver`] goes to the real store,
//! except the calls a [`Faults`] rule answers with an error first. A rule
//! sees the method's name and how many times it was called before (0 for
//! the first call), so "the second `get`" or "every `tenant_bytes`" is one
//! closure.

use antares_model::{NgsiError, TenantId};
use antares_store::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type Rule = Box<dyn Fn(&str, usize) -> Option<NgsiError> + Send + Sync>;

/// The rule and the per-method call counts one driver keeps.
pub struct Faults {
    rule: Rule,
    calls: Mutex<HashMap<String, usize>>,
}

impl Faults {
    pub fn new(rule: impl Fn(&str, usize) -> Option<NgsiError> + Send + Sync + 'static) -> Self {
        Self {
            rule: Box::new(rule),
            calls: Mutex::default(),
        }
    }

    /// Counts one call of `method` and answers the rule's error, if any.
    pub fn hit(&self, method: &str) -> Result<(), NgsiError> {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = calls.entry(method.to_owned()).or_default();
        let before = *n;
        *n += 1;
        drop(calls);
        (self.rule)(method, before).map_or(Ok(()), Err)
    }

    /// How many times `method` was called.
    pub fn calls(&self, method: &str) -> usize {
        let calls = self
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        calls.get(method).copied().unwrap_or(0)
    }
}

/// [`CurrentStateDriver`] over `inner`, failing what `faults` says.
pub struct StoreFaults {
    pub inner: Arc<dyn CurrentStateDriver>,
    pub faults: Faults,
}

/// [`TemporalDriver`] over `inner`, failing what `faults` says.
pub struct TemporalFaults {
    pub inner: Arc<dyn TemporalDriver>,
    pub faults: Faults,
}

/// A store error a fault answers with: the generic detail a backend gives.
pub fn store_down() -> NgsiError {
    NgsiError::InternalError("store unavailable".into())
}

#[async_trait::async_trait]
impl CurrentStateDriver for StoreFaults {
    async fn ping(&self) -> Result<(), NgsiError> {
        self.faults.hit("ping")?;
        self.inner.ping().await
    }
    async fn wedged(&self) -> bool {
        self.inner.wedged().await
    }
    fn commit_queue(&self) -> Option<(usize, usize)> {
        self.inner.commit_queue()
    }
    async fn close(&self) {
        self.inner.close().await
    }
    fn version_info(&self) -> Value {
        self.inner.version_info()
    }
    fn set_change_hook(&self, h: ChangeHook) {
        self.inner.set_change_hook(h)
    }
    fn set_outbox(&self, on: bool) {
        self.inner.set_outbox(on)
    }
    async fn outbox_peek(&self, limit: i64) -> Result<Vec<(i64, String, Value)>, NgsiError> {
        self.faults.hit("outbox_peek")?;
        self.inner.outbox_peek(limit).await
    }
    async fn outbox_ack(&self, seqs: &[i64]) -> Result<u64, NgsiError> {
        self.faults.hit("outbox_ack")?;
        self.inner.outbox_ack(seqs).await
    }
    async fn outbox_retain(&self, tenant: &TenantId, seqs: &[i64]) -> Result<u64, NgsiError> {
        self.faults.hit("outbox_retain")?;
        self.inner.outbox_retain(tenant, seqs).await
    }
    async fn outbox_event(&self, seq: i64, tenant: &TenantId) -> Result<Option<Value>, NgsiError> {
        self.faults.hit("outbox_event")?;
        self.inner.outbox_event(seq, tenant).await
    }
    async fn create(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        self.faults.hit("create")?;
        self.inner.create(tenant, kind, id, doc).await
    }
    async fn batch_create(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        self.faults.hit("batch_create")?;
        self.inner.batch_create(tenant, items).await
    }
    async fn batch_delete(
        &self,
        tenant: &TenantId,
        ids: &[String],
    ) -> Result<Vec<bool>, NgsiError> {
        self.faults.hit("batch_delete")?;
        self.inner.batch_delete(tenant, ids).await
    }
    async fn batch_upsert(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        self.faults.hit("batch_upsert")?;
        self.inner.batch_upsert(tenant, items).await
    }
    async fn upsert(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        self.faults.hit("upsert")?;
        self.inner.upsert(tenant, kind, id, doc).await
    }
    async fn get(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        self.faults.hit("get")?;
        self.inner.get(tenant, kind, id).await
    }
    async fn get_entities(
        &self,
        tenant: &TenantId,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Value>, NgsiError> {
        self.faults.hit("get_entities")?;
        self.inner.get_entities(tenant, ids).await
    }
    async fn delete(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<bool, NgsiError> {
        self.faults.hit("delete")?;
        self.inner.delete(tenant, kind, id).await
    }
    async fn delete_entity_if(
        &self,
        tenant: &TenantId,
        id: &str,
        keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
    ) -> Result<bool, NgsiError> {
        self.faults.hit("delete_entity_if")?;
        self.inner.delete_entity_if(tenant, id, keep).await
    }
    async fn list(&self, tenant: &TenantId, kind: Kind) -> Result<Vec<Value>, NgsiError> {
        self.faults.hit("list")?;
        self.inner.list(tenant, kind).await
    }
    async fn list_digests(
        &self,
        tenant: &TenantId,
        kind: Kind,
    ) -> Result<Vec<(String, u64)>, NgsiError> {
        self.faults.hit("list_digests")?;
        self.inner.list_digests(tenant, kind).await
    }
    async fn doc_revision(
        &self,
        _tenant: &TenantId,
        _kind: Kind,
    ) -> Result<Option<u64>, NgsiError> {
        self.faults.hit("doc_revision")?;
        self.inner.doc_revision(_tenant, _kind).await
    }
    async fn list_page(
        &self,
        tenant: &TenantId,
        kind: Kind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, NgsiError> {
        self.faults.hit("list_page")?;
        self.inner.list_page(tenant, kind, after, limit).await
    }
    async fn list_slice(
        &self,
        tenant: &TenantId,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Value>, usize), NgsiError> {
        self.faults.hit("list_slice")?;
        self.inner.list_slice(tenant, kind, offset, limit).await
    }
    async fn matching_registrations(
        &self,
        tenant: &TenantId,
        ids: Option<&[String]>,
        types: Option<&[String]>,
    ) -> Result<Vec<Value>, NgsiError> {
        self.faults.hit("matching_registrations")?;
        self.inner.matching_registrations(tenant, ids, types).await
    }
    async fn query_entities(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome, NgsiError> {
        self.faults.hit("query_entities")?;
        self.inner.query_entities(tenant, f).await
    }
    async fn query_candidates(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome, NgsiError> {
        self.faults.hit("query_candidates")?;
        self.inner.query_candidates(tenant, f).await
    }
    async fn query_entities_text(
        &self,
        tenant: &TenantId,
        f: &filter::EntityFilter<'_>,
    ) -> Result<filter::QueryOutcome<Box<serde_json::value::RawValue>>, NgsiError> {
        self.faults.hit("query_entities_text")?;
        self.inner.query_entities_text(tenant, f).await
    }
    async fn entity_type_names(&self, tenant: &TenantId) -> Result<Option<Vec<String>>, NgsiError> {
        self.faults.hit("entity_type_names")?;
        self.inner.entity_type_names(tenant).await
    }
    async fn entity_types_beside(
        &self,
        tenant: &TenantId,
        any_of: &[String],
    ) -> Result<Option<Vec<String>>, NgsiError> {
        self.faults.hit("entity_types_beside")?;
        self.inner.entity_types_beside(tenant, any_of).await
    }
    async fn remapped_id(&self, tenant: &TenantId, id: &str) -> Result<Option<String>, NgsiError> {
        self.faults.hit("remapped_id")?;
        self.inner.remapped_id(tenant, id).await
    }
    async fn entity_id_segments(
        &self,
        tenant: &TenantId,
        lead: &str,
        sep: char,
        max: usize,
    ) -> Result<Option<Vec<String>>, NgsiError> {
        self.faults.hit("entity_id_segments")?;
        self.inner.entity_id_segments(tenant, lead, sep, max).await
    }
    async fn entity_attr_pairs(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<Vec<(String, String)>>, NgsiError> {
        self.faults.hit("entity_attr_pairs")?;
        self.inner.entity_attr_pairs(tenant).await
    }
    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.faults.hit("mutate_boxed")?;
        self.inner.mutate_boxed(tenant, kind, id, f).await
    }
    async fn batch_mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        f: BatchMutateFn<'a>,
    ) -> Result<Vec<Option<Result<(), ()>>>, NgsiError> {
        self.faults.hit("batch_mutate_boxed")?;
        self.inner.batch_mutate_boxed(tenant, ids, f).await
    }
    async fn record_delivery(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        now: &str,
    ) -> Result<Option<Delivery>, NgsiError> {
        self.faults.hit("record_delivery")?;
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
        self.faults.hit("record_failure")?;
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
        self.faults.hit("record_deliveries")?;
        self.inner.record_deliveries(tenant, kind, attempts).await
    }
    async fn confirmation_ask(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _question: &confirmations::Confirmation,
        _caps: confirmations::Caps,
    ) -> Result<bool, NgsiError> {
        self.faults.hit("confirmation_ask")?;
        self.inner
            .confirmation_ask(_tenant, _id, _question, _caps)
            .await
    }
    async fn confirmation_find(
        &self,
        _tenant: &TenantId,
        _id: &str,
    ) -> Result<Option<confirmations::Confirmation>, NgsiError> {
        self.faults.hit("confirmation_find")?;
        self.inner.confirmation_find(_tenant, _id).await
    }
    async fn confirmation_decide(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _accept: bool,
    ) -> Result<Option<confirmations::Confirmation>, NgsiError> {
        self.faults.hit("confirmation_decide")?;
        self.inner.confirmation_decide(_tenant, _id, _accept).await
    }
    async fn confirmation_take(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _subject: &confirmations::Subject,
    ) -> Result<confirmations::Taken, NgsiError> {
        self.faults.hit("confirmation_take")?;
        self.inner.confirmation_take(_tenant, _id, _subject).await
    }
    async fn confirmation_spend(
        &self,
        _tenant: &TenantId,
        _id: &str,
        _subject: &confirmations::Subject,
        _ttl: std::time::Duration,
        _caps: confirmations::Caps,
    ) -> Result<bool, NgsiError> {
        self.faults.hit("confirmation_spend")?;
        self.inner
            .confirmation_spend(_tenant, _id, _subject, _ttl, _caps)
            .await
    }
    async fn confirmation_sweep(&self, _tenant: &TenantId) -> Result<u64, NgsiError> {
        self.faults.hit("confirmation_sweep")?;
        self.inner.confirmation_sweep(_tenant).await
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
        self.faults.hit("retain_current")?;
        self.inner
            .retain_current(tenant, type_iri, before, batch, dry_run)
            .await
    }
    async fn history_overrides(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, Value)>, NgsiError> {
        self.faults.hit("history_overrides")?;
        self.inner.history_overrides(limit).await
    }
    async fn record_forward(
        &self,
        tenant: &TenantId,
        id: &str,
        now: &str,
        ok: bool,
    ) -> Result<Option<Value>, NgsiError> {
        self.faults.hit("record_forward")?;
        self.inner.record_forward(tenant, id, now, ok).await
    }
    async fn tenant_exists(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        self.faults.hit("tenant_exists")?;
        self.inner.tenant_exists(tenant).await
    }
    async fn tenant_standing(&self, _tenant: &TenantId) -> Result<TenantStanding, NgsiError> {
        self.faults.hit("tenant_standing")?;
        self.inner.tenant_standing(_tenant).await
    }
    async fn subscription_tenants(&self) -> Result<Vec<String>, NgsiError> {
        self.faults.hit("subscription_tenants")?;
        self.inner.subscription_tenants().await
    }
    async fn tenant_ids(&self) -> Result<Vec<String>, NgsiError> {
        self.faults.hit("tenant_ids")?;
        self.inner.tenant_ids().await
    }
    async fn tenant_stats_one(&self, tenant: &TenantId) -> Result<Option<TenantStats>, NgsiError> {
        self.faults.hit("tenant_stats_one")?;
        self.inner.tenant_stats_one(tenant).await
    }
    async fn tenant_bytes(&self, tenant: &TenantId) -> Result<TenantBytes, NgsiError> {
        self.faults.hit("tenant_bytes")?;
        self.inner.tenant_bytes(tenant).await
    }
    async fn tenant_bytes_estimate(
        &self,
        tenant: &TenantId,
        entities: u64,
    ) -> Result<TenantBytes, NgsiError> {
        self.faults.hit("tenant_bytes_estimate")?;
        self.inner.tenant_bytes_estimate(tenant, entities).await
    }
    async fn purge_tenant(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        self.faults.hit("purge_tenant")?;
        self.inner.purge_tenant(tenant).await
    }
    async fn context_put(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        doc: Value,
    ) -> Result<(), NgsiError> {
        self.faults.hit("context_put")?;
        self.inner.context_put(tenant, id, doc).await
    }
    async fn context_put_capped(
        &self,
        tenant: &TenantId,
        id: &str,
        doc: Value,
        caps: ContextCaps,
    ) -> Result<ContextPut, NgsiError> {
        self.faults.hit("context_put_capped")?;
        self.inner.context_put_capped(tenant, id, doc, caps).await
    }
    async fn context_get(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        self.faults.hit("context_get")?;
        self.inner.context_get(tenant, id).await
    }
    async fn context_mutate<'a>(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.faults.hit("context_mutate")?;
        self.inner.context_mutate(tenant, id, f).await
    }
    async fn context_delete(&self, tenant: Option<&TenantId>, id: &str) -> Result<bool, NgsiError> {
        self.faults.hit("context_delete")?;
        self.inner.context_delete(tenant, id).await
    }
    async fn context_list_meta(&self, tenant: Option<&TenantId>) -> Result<Vec<Value>, NgsiError> {
        self.faults.hit("context_list_meta")?;
        self.inner.context_list_meta(tenant).await
    }
}

#[async_trait::async_trait]
impl TemporalDriver for TemporalFaults {
    async fn event_list(&self, evs: &[TemporalEvent]) -> Result<(), NgsiError> {
        self.faults.hit("event_list")?;
        self.inner.event_list(evs).await
    }
    fn supported(&self) -> bool {
        self.inner.supported()
    }
    async fn attr_instance_count(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        self.faults.hit("attr_instance_count")?;
        self.inner.attr_instance_count(tenant).await
    }
    async fn attr_instance_count_within(
        &self,
        tenant: &TenantId,
        bound: std::time::Duration,
    ) -> Result<u64, NgsiError> {
        self.faults.hit("attr_instance_count_within")?;
        self.inner.attr_instance_count_within(tenant, bound).await
    }
    async fn attr_instance_bytes(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        self.faults.hit("attr_instance_bytes")?;
        self.inner.attr_instance_bytes(tenant).await
    }
    async fn attr_instance_usage(
        &self,
        tenant: &TenantId,
        bound: std::time::Duration,
    ) -> Result<(u64, u64), NgsiError> {
        self.faults.hit("attr_instance_usage")?;
        self.inner.attr_instance_usage(tenant, bound).await
    }
    async fn attr_instance_estimate(&self, tenant: &TenantId) -> Result<(u64, u64), NgsiError> {
        self.faults.hit("attr_instance_estimate")?;
        self.inner.attr_instance_estimate(tenant).await
    }
    async fn retain_history(
        &self,
        tenant: &TenantId,
        scope: HistoryScope<'_>,
        before: &str,
        batch: usize,
        dry_run: bool,
    ) -> Result<u64, NgsiError> {
        self.faults.hit("retain_history")?;
        self.inner
            .retain_history(tenant, scope, before, batch, dry_run)
            .await
    }
    async fn drop_retained_history(
        &self,
        floors: &[(String, String)],
    ) -> Result<Vec<String>, NgsiError> {
        self.faults.hit("drop_retained_history")?;
        self.inner.drop_retained_history(floors).await
    }
    async fn retain_audit(&self, before: &str, batch: usize) -> Result<u64, NgsiError> {
        self.faults.hit("retain_audit")?;
        self.inner.retain_audit(before, batch).await
    }
    fn set_short_history(&self, short: ShortHistory) {
        self.inner.set_short_history(short)
    }
    async fn versions(
        &self,
        tenant: &TenantId,
        entity_id: &str,
    ) -> Result<serde_json::Map<String, Value>, NgsiError> {
        self.faults.hit("versions")?;
        self.inner.versions(tenant, entity_id).await
    }
    async fn erase(
        &self,
        tenant: &TenantId,
        entity_id: &str,
        attrs: Option<&[String]>,
        before: Option<&str>,
    ) -> Result<u64, NgsiError> {
        self.faults.hit("erase")?;
        self.inner.erase(tenant, entity_id, attrs, before).await
    }
    async fn purge_tenant(&self, tenant: &TenantId) -> Result<(), NgsiError> {
        self.faults.hit("purge_tenant")?;
        self.inner.purge_tenant(tenant).await
    }
    async fn ping(&self) -> Result<(), NgsiError> {
        self.faults.hit("ping")?;
        self.inner.ping().await
    }
    fn version_info(&self) -> Value {
        self.inner.version_info()
    }
    async fn close(&self) {
        self.inner.close().await
    }
    async fn temporal_append(
        &self,
        tenant: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        self.faults.hit("temporal_append")?;
        self.inner
            .temporal_append(tenant, id, shell, additions)
            .await
    }
    fn q_pushdown_exact(
        &self,
        _q: &antares_ql::QNode,
        _range: Option<&filter::InstanceRange<'_>>,
        _expand: &dyn Fn(&str) -> String,
    ) -> bool {
        self.inner.q_pushdown_exact(_q, _range, _expand)
    }
    async fn query_temporal(
        &self,
        tenant: &TenantId,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError> {
        self.faults.hit("query_temporal")?;
        self.inner.query_temporal(tenant, f).await
    }
    async fn get_temporal(
        &self,
        tenant: &TenantId,
        id: &str,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        self.faults.hit("get_temporal")?;
        self.inner.get_temporal(tenant, id, f).await
    }
    async fn get(&self, tenant: &TenantId, id: &str) -> Result<Option<Value>, NgsiError> {
        self.faults.hit("get")?;
        self.inner.get(tenant, id).await
    }
    async fn create(&self, tenant: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.faults.hit("create")?;
        self.inner.create(tenant, id, doc).await
    }
    async fn upsert(&self, tenant: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.faults.hit("upsert")?;
        self.inner.upsert(tenant, id, doc).await
    }
    async fn delete(&self, tenant: &TenantId, id: &str) -> Result<bool, NgsiError> {
        self.faults.hit("delete")?;
        self.inner.delete(tenant, id).await
    }
    async fn list(&self, tenant: &TenantId) -> Result<Vec<Value>, NgsiError> {
        self.faults.hit("list")?;
        self.inner.list(tenant).await
    }
    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.faults.hit("mutate_boxed")?;
        self.inner.mutate_boxed(tenant, id, f).await
    }
}
