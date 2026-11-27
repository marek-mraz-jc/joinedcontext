// SPDX-License-Identifier: EUPL-1.2
//! Temporal storage as a driver choice: a broker composed WITHOUT a
//! temporal store answers the temporal API with the error type
//! OperationNotSupported and HTTP 422 (CIM 009 Table 6.3.2-1:
//! https://uri.etsi.org/ngsi-ld/errors/OperationNotSupported -> 422),
//! records no history, and leaves the current-state API untouched.
//!
//! Both directions asserted: the refusal must be exactly the mapped error
//! (not 404, not 500, no partial data), and the same requests against the
//! default composition (fused store) must NOT be refused — proving the
//! behaviour comes from the driver choice, not from the temporal routes.

use antares_api::AppState;
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;
use antares_store::NoTemporal;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn state_without_temporal() -> AppState {
    let store = Arc::new(AnyStore::Mem(Store::default()));
    AppState::with_drivers("me".into(), store, Arc::new(NoTemporal), "memory")
}

async fn req(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    let body = match body {
        Some(v) => {
            let s = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let resp = antares_api::router(st.clone())
        .oneshot(b.body(body).expect("req"))
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn assert_unsupported(status: StatusCode, body: &Value) {
    assert_eq!(
        status, 422,
        "Table 6.3.2-1 maps OperationNotSupported to 422"
    );
    assert_eq!(
        body["type"], "https://uri.etsi.org/ngsi-ld/errors/OperationNotSupported",
        "the problem type must be the spec's OperationNotSupported URI: {body}"
    );
    assert!(
        body.get("data").is_none() && body.get("id").is_none(),
        "a refusal must carry no partial temporal data: {body}"
    );
}

#[tokio::test]
async fn temporal_reads_answer_operation_not_supported() {
    let st = state_without_temporal();
    let (status, body) = req(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=T&timerel=after&timeAt=2020-08-01T12:00:00Z",
        None,
    )
    .await;
    assert_unsupported(status, &body);
    let (status, body) = req(&st, "GET", "/ngsi-ld/v1/temporal/entities/urn:x:1", None).await;
    assert_unsupported(status, &body);
}

#[tokio::test]
async fn current_state_still_works_and_records_no_history() {
    let st = state_without_temporal();
    assert!(!st.record_locally(), "NoTemporal means nothing records");
    let (status, _) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": "urn:x:1", "type": "T",
                    "speed": {"type": "Property", "value": 3}})),
    )
    .await;
    assert_eq!(
        status, 201,
        "entity provision is untouched by the driver choice"
    );
    let (status, body) = req(&st, "GET", "/ngsi-ld/v1/entities/urn:x:1", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["id"], "urn:x:1");
    // the recorder produced nothing — the driver holds no doc for the entity
    let t = antares_model::TenantId::new(antares_model::TenantId::DEFAULT).expect("tenant");
    assert_eq!(
        st.temporal.get(&t, "urn:x:1").await.expect("driver ok"),
        None
    );
}

/// A driver that would page a temporal query by its `q`: it records every
/// name the broker's expander gives it, for the pushdown verdict and for the
/// read, and delegates the read.
struct Expanding {
    inner: Arc<dyn antares_store::TemporalDriver>,
    expanded: std::sync::Mutex<Vec<String>>,
}

impl Expanding {
    fn note(&self, iri: String) {
        self.expanded.lock().expect("lock").push(iri);
    }
}

#[async_trait::async_trait]
impl antares_store::TemporalDriver for Expanding {
    fn q_pushdown_exact(
        &self,
        _q: &antares_ql::QNode,
        _range: Option<&antares_store::filter::InstanceRange<'_>>,
        expand: &dyn Fn(&str) -> String,
    ) -> bool {
        self.note(expand("speed"));
        false
    }
    async fn query_temporal(
        &self,
        t: &antares_model::TenantId,
        f: &antares_store::filter::TemporalFilter<'_>,
    ) -> Result<antares_store::filter::TemporalOutcome, antares_model::NgsiError> {
        self.note((f.expand)("speed"));
        self.inner.query_temporal(t, f).await
    }
    async fn get_temporal(
        &self,
        t: &antares_model::TenantId,
        id: &str,
        f: &antares_store::filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, antares_model::NgsiError> {
        self.inner.get_temporal(t, id, f).await
    }
    async fn temporal_append(
        &self,
        t: &antares_model::TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), antares_model::NgsiError> {
        self.inner.temporal_append(t, id, shell, additions).await
    }
    async fn get(
        &self,
        t: &antares_model::TenantId,
        id: &str,
    ) -> Result<Option<Value>, antares_model::NgsiError> {
        self.inner.get(t, id).await
    }
    async fn create(
        &self,
        t: &antares_model::TenantId,
        id: &str,
        doc: Value,
    ) -> Result<bool, antares_model::NgsiError> {
        self.inner.create(t, id, doc).await
    }
    async fn upsert(
        &self,
        t: &antares_model::TenantId,
        id: &str,
        doc: Value,
    ) -> Result<bool, antares_model::NgsiError> {
        self.inner.upsert(t, id, doc).await
    }
    async fn delete(
        &self,
        t: &antares_model::TenantId,
        id: &str,
    ) -> Result<bool, antares_model::NgsiError> {
        self.inner.delete(t, id).await
    }
    async fn list(
        &self,
        t: &antares_model::TenantId,
    ) -> Result<Vec<Value>, antares_model::NgsiError> {
        self.inner.list(t).await
    }
    async fn mutate_boxed<'a>(
        &self,
        t: &antares_model::TenantId,
        id: &str,
        f: antares_store::MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, antares_model::NgsiError> {
        self.inner.mutate_boxed(t, id, f).await
    }
}

/// A temporal query's `q` reaches a driver that pages by it through the
/// request's own @context: the pushdown verdict and the read both expand
/// `speed` to its full name.
#[tokio::test]
async fn a_temporal_q_reaches_the_driver_expanded_by_the_request_context() {
    let base = AppState::new("me".into());
    let driver = Arc::new(Expanding {
        inner: base.temporal.clone(),
        expanded: std::sync::Mutex::default(),
    });
    let st = AppState::with_drivers("me".into(), base.store.clone(), driver.clone(), "memory");
    let (status, body) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/temporal/entities",
        Some(
            json!({"id": "urn:ngsi-ld:Vehicle:q", "type": "Vehicle", "speed": [
            {"type": "Property", "value": 5, "observedAt": "2026-01-01T00:00:00Z"}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = req(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Vehicle&q=speed%3E1&timerel=after&timeAt=2025-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");
    let seen = driver.expanded.lock().expect("lock").clone();
    assert!(seen.len() >= 2, "{seen:?}");
    assert!(
        seen.iter()
            .all(|s| s == "https://uri.etsi.org/ngsi-ld/default-context/speed"),
        "{seen:?}"
    );
}

#[tokio::test]
async fn the_default_composition_is_not_refused() {
    // control: same requests against the fused store — anything but the
    // unsupported refusal proves the 422 above comes from the driver choice
    let st = AppState::new("me".into());
    let (status, _) = req(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=T&timerel=after&timeAt=2020-08-01T12:00:00Z",
        None,
    )
    .await;
    assert_ne!(status, 422, "a temporal-capable broker must not refuse");
    let (status, _) = req(&st, "GET", "/ngsi-ld/v1/temporal/entities/urn:x:1", None).await;
    assert_ne!(status, 422);
}
