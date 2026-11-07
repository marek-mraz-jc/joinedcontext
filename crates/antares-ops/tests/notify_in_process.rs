// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rule 7: `Ops::notify_in_process` posts a notification through
//! the broker's own router with no subject, and a receiver that does not
//! take it is a failed attempt; `Ops::notification_receiver` reads the
//! inbound index, never the notification. One binary, because it sets the
//! subject header list in the environment before the broker first reads it.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::{BuiltForward, ForwardFuture, ForwardTransport, Ops};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};

/// Serves no forward; the test keeps the `Ops` it was handed.
struct Keep;

impl ForwardTransport for Keep {
    fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        None
    }
}

/// A decision: its Tenant and the subject header list.
type Seen = (String, Vec<(String, String)>);

/// Records every decision.
#[derive(Default)]
struct Witness(Mutex<Vec<Seen>>);

impl PolicyEngine for Witness {
    fn name(&self) -> &'static str {
        "witness"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((s.tenant.as_str().to_owned(), s.headers.clone()));
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// The `Ops` a transport is handed, over a broker asking `witness`.
async fn ops(witness: Arc<Witness>) -> Ops {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "X-Subject");
    let mut state = antares_api::AppState::new("antares-test".into());
    antares_api::wire(&mut state).await;
    let state = state.with_policy(witness);
    let kept: Arc<Mutex<Option<Ops>>> = Arc::default();
    let slot = kept.clone();
    let _state = antares_ops::with_forward_transport(state, move |ops| {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(ops);
        Arc::new(Keep)
    })
    .unwrap_or_else(|e| unreachable!("{e}"));
    let ops = kept.lock().unwrap_or_else(PoisonError::into_inner).take();
    ops.unwrap_or_else(|| unreachable!("the transport was not made"))
}

/// The request reaches the router, in the default Tenant, and the engine
/// is asked about nobody: no subject header travels.
#[tokio::test]
async fn an_in_process_notification_is_posted_without_a_subject() {
    let witness = Arc::new(Witness::default());
    let ops = ops(witness.clone()).await;
    let posted = ops
        .notify_in_process(
            "/ngsi-ld/v1/entities",
            &json!({"id": "urn:ngsi-ld:Car:n1", "type": "Car"}),
        )
        .await;
    assert_eq!(posted, Ok(()));
    let seen = witness
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(seen, [("default".to_owned(), Vec::new())]);
}

/// A notification no distributed Subscription maps: no receiver, and the
/// receiver path answers 404, which is a failed attempt.
#[tokio::test]
async fn an_unmapped_notification_is_a_failed_attempt() {
    let ops = ops(Arc::default()).await;
    let copy = "urn:ngsi-ld:Subscription:distsub:unknown";
    assert_eq!(ops.notification_receiver(copy).await, None);
    let posted = ops
        .notify_in_process(
            "/ex/v1/remote-notify",
            &json!({"type": "Notification", "subscriptionId": copy, "data": []}),
        )
        .await;
    assert_eq!(posted, Err("the receiver answered 404".to_owned()));
}
