// SPDX-License-Identifier: EUPL-1.2
//! clause 5.8.1, item 4 and clause 5.8.2, item 4 with the distributed mapping unreadable: a store that
//! refuses the mapping of one Subscription at an exact read neither stamps,
//! updates nor forwards anything on a guess.

use crate::common::{self, DocCall};

use antares_api::AppState;
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const OWN: &str = "urn:ngsi-ld:Subscription:unread-mapping";

/// Reads of the mapping of [`OWN`] that fail: every one with `usize::MAX`,
/// only the n-th from the last reset with `n`, none with 0.
struct Faults {
    fail_at: AtomicUsize,
    reads: AtomicUsize,
}

impl Faults {
    fn arm(&self, at: usize) {
        self.reads.store(0, Ordering::SeqCst);
        self.fail_at.store(at, Ordering::SeqCst);
    }
}

async fn faulting() -> (
    AppState,
    Arc<Faults>,
    Arc<dyn antares_store::CurrentStateDriver>,
) {
    let faults = Arc::new(Faults {
        fail_at: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
    });
    let f = faults.clone();
    let mut st = AppState::new("antares-ds-faults".into());
    let inner = st.store.clone();
    st.store = Arc::new(common::Double::faulting_docs(
        inner.clone(),
        Arc::new(move |call| match call {
            DocCall::Get(Kind::DistSub, id) if id == OWN => {
                let n = f.reads.fetch_add(1, Ordering::SeqCst) + 1;
                let at = f.fail_at.load(Ordering::SeqCst);
                at == usize::MAX || n == at
            }
            _ => false,
        }),
    ));
    antares_api::wire(&mut st).await;
    (st, faults, inner)
}

async fn send(st: &AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let payload = body.to_string();
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json")
        .header("Content-Length", payload.len())
        .body(Body::from(payload))
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn subscribe(st: &AppState) {
    let (status, body) = send(
        st,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        json!({"id": OWN, "type": "Subscription", "entities": [{"type": "Building"}],
               "notification": {"endpoint": {"uri": "http://127.0.0.1:1/original"}}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn watch_height(st: &AppState) {
    let (status, body) = send(
        st,
        "PATCH",
        &format!("/ngsi-ld/v1/subscriptions/{OWN}"),
        json!({"watchedAttributes": ["height"]}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// The internal Registration Subscription, read past the faults.
async fn csr_doc(inner: &Arc<dyn antares_store::CurrentStateDriver>) -> Value {
    inner
        .list(&TenantId::default(), Kind::DistSub)
        .await
        .expect("list")
        .into_iter()
        .find(|d| d["id"] != OWN && d["type"] == "Subscription")
        .expect("the Registration Subscription")
}

/// clause 5.8.1, item 4 / clause 5.8.2, item 4 with every read of the mapping refused: the create
/// still makes the Registration Subscription but stamps no mapping, an
/// update leaves the Registration Subscription as it was, and a matching
/// registration is sent no copy.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_mapping_is_never_written_on_a_guess() {
    let (st, faults, inner) = faulting().await;
    faults.arm(usize::MAX);
    subscribe(&st).await;
    let csr = csr_doc(&inner).await;
    assert!(
        inner
            .get(&TenantId::default(), Kind::DistSub, OWN)
            .await
            .expect("read")
            .is_none(),
        "no mapping stamped"
    );
    watch_height(&st).await;
    assert_eq!(
        csr_doc(&inner).await.get("watchedAttributes"),
        csr.get("watchedAttributes"),
        "the update did not reach the Registration Subscription"
    );
    let (port, wire) = common::net::sink();
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        json!({"id": "urn:ngsi-ld:ContextSourceRegistration:unread", "type": "ContextSourceRegistration",
               "information": [{"entities": [{"type": "Building"}]}],
               "operations": ["createSubscription"],
               "endpoint": format!("http://127.0.0.1:{port}")}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        faults.reads.load(Ordering::SeqCst) >= 3,
        "every path read it"
    );
    assert!(
        wire.lock().expect("wire").is_empty(),
        "no copy forwarded on an unread mapping"
    );
}

/// clause 5.8.2, item 4: the mapping read after the Registration Subscription is brought
/// into step fails — the step already taken stays, and nothing is forwarded.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_stops_where_the_mapping_stops_reading() {
    let (st, faults, inner) = faulting().await;
    subscribe(&st).await;
    faults.arm(2);
    watch_height(&st).await;
    assert_eq!(
        faults.reads.load(Ordering::SeqCst),
        2,
        "read twice, the second refused"
    );
    assert_eq!(
        csr_doc(&inner).await["watchedAttributes"],
        json!(["https://uri.etsi.org/ngsi-ld/default-context/height"])
    );
}
