// SPDX-License-Identifier: EUPL-1.2
//! The driver seams through minimal drivers: the boxed mutate round-trips
//! typed results, the temporal drain folds a request, and every default a
//! driver does not override answers as the trait documents it (an empty
//! answer for bookkeeping, `OperationNotSupported` for what a client asked).

use antares_model::{NgsiError, TenantId};
use antares_store::*;
use serde_json::Value;

/// A minimal driver proving the boxed seam round-trips typed results:
/// present row → the closure's T and E cross intact; absent → None. It
/// overrides none of the trait's defaults, so it also shows what they answer;
/// its batch mutate breaks the contract on purpose.
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
    async fn batch_delete(&self, _t: &TenantId, _ids: &[String]) -> Result<Vec<bool>, NgsiError> {
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
    async fn get(&self, _t: &TenantId, _k: Kind, _id: &str) -> Result<Option<Value>, NgsiError> {
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
        Ok(self.0.lock().expect("lock").clone().into_iter().collect())
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
        Ok(filter::QueryOutcome {
            rows: self.0.lock().expect("lock").clone().into_iter().collect(),
            decided: true,
            paged: false,
            total: Some(1),
        })
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
        // a driver breaking the contract: every id rejected, the closure never called
        Ok(_ids.iter().map(|_| Some(Err(()))).collect())
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
    async fn context_put_capped(
        &self,
        _t: &TenantId,
        _id: &str,
        _doc: Value,
        _caps: ContextCaps,
    ) -> Result<ContextPut, NgsiError> {
        Ok(ContextPut::Stored)
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
    async fn context_delete(&self, _t: Option<&TenantId>, _id: &str) -> Result<bool, NgsiError> {
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
    // a count of its own, so the defaults built on it show they read it
    async fn attr_instance_count(&self, _t: &TenantId) -> Result<u64, NgsiError> {
        Ok(7)
    }
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

fn tenant() -> TenantId {
    TenantId::new("t").expect("tenant")
}

fn not_supported<T: std::fmt::Debug>(r: Result<T, NgsiError>, what: &str) {
    let e = r.expect_err(what);
    assert_eq!(e.kind(), "OperationNotSupported", "{what}: {e:?}");
    assert_eq!(e.status(), 422, "{what}");
}

/// A driver that overrides nothing beyond the required methods: its
/// bookkeeping defaults are benign and empty, the inventory, size, purge and
/// confirmation defaults refuse as not supported, and the defaults built on
/// the required reads answer from them.
#[tokio::test]
async fn the_current_state_defaults_answer_as_documented() {
    let t = tenant();
    let d: std::sync::Arc<dyn CurrentStateDriver> =
        std::sync::Arc::new(OneDoc(std::sync::Mutex::new(None)));
    assert_eq!(d.version_info(), serde_json::json!({}));
    d.set_outbox(true);
    assert!(d.outbox_peek(10).await.expect("peek").is_empty());
    assert_eq!(d.outbox_ack(&[1, 2]).await.expect("ack"), 0);
    assert_eq!(d.outbox_retain(&t, &[1]).await.expect("retain"), 0);
    assert_eq!(d.outbox_event(1, &t).await.expect("event"), None);
    assert_eq!(d.doc_revision(&t, Kind::Entity).await.expect("rev"), None);
    assert_eq!(
        d.entity_types_beside(&t, &["T".to_owned()])
            .await
            .expect("beside"),
        None
    );
    assert_eq!(
        d.entity_id_segments(&t, "urn:", ':', 5)
            .await
            .expect("segments"),
        None
    );
    assert_eq!(d.confirmation_sweep(&t).await.expect("sweep"), 0);
    assert_eq!(d.sweep_expired().await, 0);
    assert_eq!(
        d.retain_current(&t, "T", "2026-01-01T00:00:00Z", 100, true)
            .await
            .expect("retain"),
        0
    );
    assert!(d.history_overrides(10).await.expect("overrides").is_empty());
    assert_eq!(
        d.tenant_standing(&t).await.expect("standing"),
        TenantStanding::default()
    );
    not_supported(d.tenant_ids().await, "tenant_ids");
    not_supported(d.tenant_stats_one(&t).await, "tenant_stats_one");
    not_supported(d.tenant_bytes(&t).await, "tenant_bytes");
    not_supported(
        d.tenant_bytes_estimate(&t, 10).await,
        "tenant_bytes_estimate",
    );
    not_supported(d.purge_tenant(&t).await, "purge_tenant");

    let question = confirmations::Confirmation {
        view: "v".into(),
        tool: "update".into(),
        message: "sure?".into(),
        subject: Vec::new(),
        left: std::time::Duration::from_secs(60),
        decision: None,
    };
    let caps = confirmations::Caps {
        tenant: 10,
        subject: 2,
    };
    // a store without confirmations fails the call, never pretends one was kept
    let refused = |e: NgsiError| e.status() == 500;
    assert!(d
        .confirmation_ask(&t, "c1", &question, caps)
        .await
        .is_err_and(refused));
    assert!(d.confirmation_find(&t, "c1").await.is_err_and(refused));
    assert!(d
        .confirmation_decide(&t, "c1", true)
        .await
        .is_err_and(refused));
    assert!(d
        .confirmation_take(&t, "c1", &Vec::new())
        .await
        .is_err_and(refused));
    assert!(d
        .confirmation_spend(
            &t,
            "c1",
            &Vec::new(),
            std::time::Duration::from_secs(1),
            caps
        )
        .await
        .is_err_and(refused));

    // the defaults over the required reads
    assert!(d
        .list_digests(&t, Kind::Entity)
        .await
        .expect("digests")
        .is_empty());
    d.create(
        &t,
        Kind::Entity,
        "urn:a",
        serde_json::json!({"id": "urn:a", "n": 1}),
    )
    .await
    .expect("create");
    let digests = d.list_digests(&t, Kind::Entity).await.expect("digests");
    assert_eq!(digests.len(), 1);
    assert_eq!(digests[0].0, "urn:a");
    let f = filter::EntityFilter::default();
    let candidates = d.query_candidates(&t, &f).await.expect("candidates");
    assert_eq!(candidates.rows.len(), 1);
    assert_eq!(candidates.total, Some(1));
    let text = d.query_entities_text(&t, &f).await.expect("text");
    assert_eq!(text.rows.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(text.rows[0].get()).expect("json")["n"],
        1
    );
    assert!(text.decided && !text.paged);
}

/// The failure stamp through the default `record_failure`: status failed on
/// the subscription and its notification, `lastSuccess` rolled back to what
/// it was, `timesFailed` moved by one; with no earlier success it is
/// removed. A missing subscription is `None`.
#[tokio::test]
async fn the_default_failure_stamp_rolls_back_last_success() {
    let t = tenant();
    let d = OneDoc(std::sync::Mutex::new(None));
    let none = d
        .record_failure(&t, Kind::Subscription, "s", None, "now")
        .await
        .expect("absent");
    assert!(none.is_none());
    d.create(
        &t,
        Kind::Subscription,
        "s",
        serde_json::json!({"id": "s", "notification": {"lastSuccess": "t2", "timesFailed": 2}}),
    )
    .await
    .expect("create");
    let doc = d
        .record_failure(&t, Kind::Subscription, "s", Some(Value::from("t1")), "t3")
        .await
        .expect("stamp")
        .expect("present");
    assert_eq!(doc["status"], "failed");
    let n = &doc["notification"];
    assert_eq!(n["lastSuccess"], "t1");
    assert_eq!(n["lastFailure"], "t3");
    assert_eq!(n["timesFailed"], 3);
    assert_eq!(n["status"], "failed");
    let doc = d
        .record_failure(&t, Kind::Subscription, "s", None, "t4")
        .await
        .expect("stamp")
        .expect("present");
    assert!(doc["notification"].get("lastSuccess").is_none(), "{doc}");
    assert_eq!(doc["notification"]["timesFailed"], 4);
}

/// A stored document that is no JSON object (a broken backend) is stamped
/// with nothing and handed back as it is: a delivery, a failure and a
/// forward never panic on it.
#[tokio::test]
async fn the_stamps_leave_a_document_that_is_no_object_alone() {
    let t = tenant();
    let d = OneDoc(std::sync::Mutex::new(Some(serde_json::json!([
        "not", "an", "object"
    ]))));
    let delivered = record_delivery_via_mutate(&d, &t, Kind::Subscription, "s", "now")
        .await
        .expect("ok")
        .expect("present");
    assert_eq!(delivered.doc, serde_json::json!(["not", "an", "object"]));
    assert_eq!(delivered.prev_success, None);
    let failed = record_failure_via_mutate(&d, &t, Kind::Subscription, "s", None, "now")
        .await
        .expect("ok")
        .expect("present");
    assert_eq!(failed, serde_json::json!(["not", "an", "object"]));
    for ok in [true, false] {
        let forwarded = record_forward_via_mutate(&d, &t, "r", "now", ok)
            .await
            .expect("ok")
            .expect("present");
        assert_eq!(forwarded, serde_json::json!(["not", "an", "object"]));
    }
}

/// `stored_object` fails the one request on a document that is no object,
/// with a generic internal error, and hands back the map of one that is.
#[test]
fn a_stored_document_must_be_an_object() {
    let mut list = serde_json::json!([1]);
    let e = stored_object(&mut list).expect_err("an array is no document");
    assert_eq!(e.status(), 500);
    let mut doc = serde_json::json!({"id": "x"});
    assert_eq!(stored_object(&mut doc).expect("an object")["id"], "x");
}

/// A row that is no object names no `kind`, so it reads as the default
/// Tenant's and passes through the view untouched; the stored form of one
/// keeps it as it is rather than losing a row it cannot read.
#[test]
fn a_context_row_that_is_no_object_passes_through() {
    let t = tenant();
    let used = serde_json::json!({"numberOfHits": 3});
    assert_eq!(
        context_row_view(Value::from("cached"), Some(&t), Some(&used)),
        Some(Value::from("cached"))
    );
    assert_eq!(
        context_row_stored(Value::from("cached"), None, Some(&t)),
        Value::from("cached")
    );
    // the boot warm reads a Cached copy without anybody's counters
    let cached = serde_json::json!({"kind": "Cached", "url": "https://x/ctx.jsonld",
        "numberOfHits": 9, "lastUsage": "2026-01-01T00:00:00Z"});
    let view = context_row_view(cached, None, None).expect("the boot warm reads every copy");
    assert_eq!(view["url"], "https://x/ctx.jsonld");
    assert!(
        view.get("numberOfHits").is_none() && view.get("lastUsage").is_none(),
        "{view}"
    );
}

/// A batch driver that reports a rejection the closure never raised has
/// broken the contract: the batch fails with an internal error instead of
/// inventing one for the caller.
#[tokio::test]
async fn a_batch_driver_reporting_unraised_rejections_fails_the_batch() {
    let t = tenant();
    let d = OneDoc(std::sync::Mutex::new(None));
    let e = d
        .batch_mutate::<&str>(&t, &["urn:a".to_owned()], |_, _| Ok(()))
        .await
        .expect_err("more rejections than raised");
    assert_eq!(e.status(), 500);
    assert!(format!("{e:?}").contains("more rejections"), "{e:?}");
    // no id, no slot: an empty batch is an empty answer
    let none = d
        .batch_mutate::<&str>(&t, &[], |_, _| Ok(()))
        .await
        .expect("empty");
    assert!(none.is_empty());
}

/// Without a history store the inventory reads zero and empty, the
/// retention passes delete nothing, an estimate is not supported, the
/// writes create nothing, and no q is pushed down.
#[tokio::test]
async fn no_temporal_answers_every_bookkeeping_call_benignly() {
    let t = tenant();
    let d: std::sync::Arc<dyn TemporalDriver> = std::sync::Arc::new(NoTemporal);
    let bound = std::time::Duration::from_millis(10);
    assert_eq!(d.attr_instance_count(&t).await.expect("count"), 0);
    assert_eq!(
        d.attr_instance_count_within(&t, bound)
            .await
            .expect("within"),
        0
    );
    assert_eq!(d.attr_instance_bytes(&t).await.expect("bytes"), 0);
    assert_eq!(
        d.attr_instance_usage(&t, bound).await.expect("usage"),
        (0, 0)
    );
    not_supported(d.attr_instance_estimate(&t).await, "estimate");
    assert_eq!(
        d.retain_history(
            &t,
            HistoryScope::Entity("urn:a"),
            "2026-01-01T00:00:00Z",
            10,
            false
        )
        .await
        .expect("retain"),
        0
    );
    assert!(d
        .drop_retained_history(&[("t".into(), "2026-01-01".into())])
        .await
        .expect("drop")
        .is_empty());
    assert_eq!(
        d.retain_audit("2026-01-01T00:00:00Z", 10)
            .await
            .expect("audit"),
        0
    );
    d.set_short_history(ShortHistory::default());
    assert!(d.versions(&t, "urn:a").await.expect("versions").is_empty());
    assert_eq!(d.erase(&t, "urn:a", None, None).await.expect("erase"), 0);
    d.purge_tenant(&t).await.expect("purge");
    d.ping().await.expect("ping");
    assert_eq!(d.version_info(), serde_json::json!({}));
    d.close().await;
    let q = antares_ql::parse_q("speed>1").expect("a q");
    assert!(!d.q_pushdown_exact(&q, None, &|s| s.to_owned()));
    assert!(!d
        .create(&t, "urn:a", serde_json::json!({}))
        .await
        .expect("create"));
    assert!(!d
        .upsert(&t, "urn:a", serde_json::json!({}))
        .await
        .expect("upsert"));
    assert_eq!(d.get(&t, "urn:a").await.expect("get"), None);
    let e = d
        .get_temporal(&t, "urn:a", &filter::TemporalFilter::default())
        .await
        .expect_err("no history to read");
    assert_eq!(e.kind(), "OperationNotSupported");
}

/// A Tenant holds at most `count` @context documents of at most `bytes` together: the count is
/// judged first, and a put that reaches the byte cap exactly still fits.
#[test]
fn context_caps_admit_up_to_both_limits() {
    let caps = ContextCaps {
        count: 2,
        bytes: 100,
    };
    assert_eq!(caps.admit(2, 0, 0), ContextPut::PastCount);
    assert_eq!(caps.admit(3, 200, 1), ContextPut::PastCount);
    assert_eq!(caps.admit(1, 0, 0), ContextPut::Stored);
    assert_eq!(caps.admit(1, 60, 40), ContextPut::Stored);
    assert_eq!(caps.admit(1, 60, 41), ContextPut::PastBytes);
    assert_eq!(caps.admit(1, u64::MAX, 1), ContextPut::PastBytes);
}

/// A row is charged its `bytes` member, else the length of the document it holds, else nothing.
#[test]
fn a_context_row_is_charged_its_bytes() {
    assert_eq!(
        context_row_bytes(&serde_json::json!({"bytes": 7, "body": {"@context": "x"}})),
        7
    );
    assert_eq!(
        context_row_bytes(&serde_json::json!({"body": {"@context": {"a": "b"}}})),
        9
    );
    assert_eq!(context_row_bytes(&serde_json::json!({})), 0);
}

/// Only the shared-database modes back several instances, and a mode prints its own name.
#[test]
fn store_modes_name_themselves_and_only_databases_are_shared() {
    for (mode, shared) in [
        (StoreMode::Memory, false),
        (StoreMode::File, false),
        (StoreMode::Postgres, true),
        (StoreMode::Timescale, true),
    ] {
        assert_eq!(mode.is_pg(), shared, "{mode}");
        assert_eq!(mode.to_string(), mode.as_str());
    }
}

/// ADR-0021: a Cached copy belongs to nobody and every call reaches it; a hosted row belongs
/// to its owner, a row without one to the default Tenant, and no other Tenant reaches it.
#[test]
fn a_context_row_belongs_to_its_owner_and_a_cached_copy_to_nobody() {
    let city = TenantId::new("city").expect("tenant");
    let other = TenantId::new("other").expect("tenant");
    let cached = serde_json::json!({"kind": "Cached"});
    let hosted = serde_json::json!({"kind": "Hosted", "owner": "city"});
    let legacy = serde_json::json!({"kind": "Hosted"});
    assert_eq!(context_row_owner(&cached), None);
    assert_eq!(context_row_owner(&hosted), Some("city"));
    assert_eq!(context_row_owner(&legacy), Some(TenantId::DEFAULT));
    assert!(context_row_visible(&cached, None));
    assert!(context_row_visible(&cached, Some(&other)));
    assert!(context_row_visible(&hosted, Some(&city)));
    assert!(!context_row_visible(&hosted, Some(&other)));
    assert!(!context_row_visible(&hosted, None));
    // a Tenant's counters on a Cached row, as a put stores them
    let row = serde_json::json!({"kind": "Cached", "usage": {"city": {"numberOfHits": 4, "lastUsage": "t"}}});
    assert_eq!(
        context_row_used(&row, Some(&city)),
        Some(&serde_json::json!({"numberOfHits": 4, "lastUsage": "t"}))
    );
    assert_eq!(context_row_used(&row, Some(&other)), None);
    assert_eq!(context_row_used(&row, None), None);
    let view = context_row_view(
        row.clone(),
        Some(&city),
        context_row_used(&row, Some(&city)),
    )
    .expect("used");
    assert_eq!(
        context_row_usage(&view),
        serde_json::json!({"numberOfHits": 4, "lastUsage": "t"})
    );
    assert_eq!(
        context_row_usage(&serde_json::json!({})),
        serde_json::json!({"numberOfHits": 0, "lastUsage": null})
    );
}

/// The remaining defaults of a driver that overrides nothing: never wedged, no commit queue, no
/// type index, no id remap, no attribute pairs; and the delivery and forward stamps through
/// `mutate`.
#[tokio::test]
async fn the_stamping_and_index_defaults_answer_as_documented() {
    let t = tenant();
    let d = OneDoc(std::sync::Mutex::new(None));
    assert!(!d.wedged().await);
    assert_eq!(d.commit_queue(), None);
    assert_eq!(d.entity_type_names(&t).await.expect("types"), None);
    assert_eq!(d.remapped_id(&t, "urn:a").await.expect("remap"), None);
    assert_eq!(d.entity_attr_pairs(&t).await.expect("pairs"), None);
    assert!(d
        .record_delivery(&t, Kind::Subscription, "s", "t1")
        .await
        .expect("absent")
        .is_none());
    d.create(
        &t,
        Kind::Subscription,
        "s",
        serde_json::json!({"id": "s", "status": "failed", "notification": {"timesSent": 1, "lastSuccess": "t0"}}),
    )
    .await
    .expect("create");
    let delivered = d
        .record_delivery(&t, Kind::Subscription, "s", "t1")
        .await
        .expect("stamp")
        .expect("present");
    assert_eq!(delivered.prev_success, Some(Value::from("t0")));
    assert!(delivered.doc.get("status").is_none(), "{}", delivered.doc);
    let n = &delivered.doc["notification"];
    assert_eq!(
        (n["timesSent"].clone(), n["lastSuccess"].clone()),
        (2.into(), "t1".into())
    );
    assert_eq!(
        (n["lastNotification"].clone(), n["status"].clone()),
        ("t1".into(), "ok".into())
    );
    // three attempts of one id book three sends; an empty list books nothing
    let booked = d
        .record_deliveries(&t, Kind::Subscription, &[("s", "t2", 3)])
        .await
        .expect("stamps");
    assert_eq!(booked.len(), 1);
    let doc = &booked[0].as_ref().expect("present").doc;
    assert_eq!(doc["notification"]["timesSent"], 5);
    assert!(d
        .record_deliveries(&t, Kind::Subscription, &[])
        .await
        .expect("none")
        .is_empty());

    let r = OneDoc(std::sync::Mutex::new(None));
    assert!(r
        .record_forward(&t, "r", "t1", true)
        .await
        .expect("absent")
        .is_none());
    r.create(&t, Kind::Registration, "r", serde_json::json!({"id": "r"}))
        .await
        .expect("create");
    let ok = r
        .record_forward(&t, "r", "t1", true)
        .await
        .expect("ok")
        .expect("present");
    assert_eq!(
        (ok["timesSent"].clone(), ok["status"].clone()),
        (1.into(), "ok".into())
    );
    assert_eq!(ok["lastSuccess"], "t1");
    assert!(ok.get("timesFailed").is_none(), "{ok}");
    let failed = r
        .record_forward(&t, "r", "t2", false)
        .await
        .expect("ok")
        .expect("present");
    assert_eq!(
        (failed["timesSent"].clone(), failed["timesFailed"].clone()),
        (2.into(), 1.into())
    );
    assert_eq!(
        (failed["lastFailure"].clone(), failed["status"].clone()),
        ("t2".into(), "failed".into())
    );
    assert_eq!(
        failed["lastSuccess"], "t1",
        "a failure keeps the last success"
    );
}

/// A temporal driver records history unless it says otherwise; without a store, the boxed
/// mutate reaches no document.
#[tokio::test]
async fn a_temporal_driver_is_supported_unless_it_keeps_none() {
    let t = tenant();
    let recorder = Recorder::default();
    assert!(recorder.supported());
    // the bounded count and the usage read the driver's own count when it keeps no other
    let bound = std::time::Duration::from_millis(10);
    assert_eq!(
        recorder
            .attr_instance_count_within(&t, bound)
            .await
            .expect("within"),
        7
    );
    assert_eq!(
        recorder
            .attr_instance_usage(&t, bound)
            .await
            .expect("usage"),
        (7, 0)
    );
    let none = NoTemporal
        .mutate_boxed(&t, "urn:a", Box::new(|_| Ok(())))
        .await
        .expect("no store");
    assert_eq!(none, None);
}

/// 4.5.6: a scope change appends to a held scope instance array, and replaces a scope that is
/// not one (plain strings) with a fresh instance array.
#[tokio::test]
async fn a_scope_change_appends_to_instances_and_replaces_anything_else() {
    let t = tenant();
    let mut scope = ev(TemporalOp::ScopeChanged, "urn:a", "scope", 0);
    scope.instance = serde_json::json!({"type": "Property", "value": "/new"});
    let d = Recorder::default();
    d.create(
        &t,
        "urn:a",
        serde_json::json!({"id": "urn:a", "scope": [{"type": "Property", "value": "/old"}]}),
    )
    .await
    .expect("create");
    d.event_list(std::slice::from_ref(&scope))
        .await
        .expect("drain");
    let doc = d.get(&t, "urn:a").await.expect("get").expect("doc");
    assert_eq!(doc["scope"].as_array().map(Vec::len), Some(2), "{doc}");
    assert_eq!(doc["scope"][1]["value"], "/new");

    let d = Recorder::default();
    d.create(
        &t,
        "urn:a",
        serde_json::json!({"id": "urn:a", "scope": ["/a", "/b"]}),
    )
    .await
    .expect("create");
    d.event_list(&[scope]).await.expect("drain");
    let doc = d.get(&t, "urn:a").await.expect("get").expect("doc");
    assert_eq!(
        doc["scope"],
        serde_json::json!([{"type": "Property", "value": "/new"}]),
        "{doc}"
    );
}

/// A store without provenance takes `@provenance` out of every instance it keeps, and only that.
#[test]
fn provenance_leaves_every_instance() {
    let mut instance = serde_json::json!({"value": 1, "@provenance": {"actor": "a"}});
    assert_eq!(
        take_provenance(&mut instance),
        Some(serde_json::json!({"actor": "a"}))
    );
    assert_eq!(instance, serde_json::json!({"value": 1}));
    assert_eq!(take_provenance(&mut instance), None);
    assert_eq!(take_provenance(&mut Value::from(3)), None);
    let mut doc = serde_json::json!({
        "id": "urn:a",
        "speed": [{"value": 1, "@provenance": {"actor": "a"}}, {"value": 2}],
        "load": [{"value": 3, "@provenance": {}}],
    });
    strip_provenance(&mut doc);
    assert_eq!(
        doc,
        serde_json::json!({"id": "urn:a", "speed": [{"value": 1}, {"value": 2}], "load": [{"value": 3}]})
    );
}
