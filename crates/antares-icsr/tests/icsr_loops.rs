// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rules 2 and 3, threats C4 and C5: a two-Tenant cycle in process
//! ends the way it ends over HTTP, and in-process forwards hold the
//! process-wide federation slots an HTTP forward holds. This binary runs
//! with two slots (`ANTARES_FED_INFLIGHT`), set before the first forward
//! reads it.

mod common;

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use common::{broker, car, cars, grant, ids, register, send, warned};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const SLOTS: usize = 2;

fn slots() {
    std::env::set_var("ANTARES_FED_INFLIGHT", SLOTS.to_string());
}

/// C4: a redirect from a to b and one from b back to a. A write forwarded
/// into b is forwarded back into a, whose Via names a already: the write
/// ends with 508 (T-3459), in process as over HTTP; a read along the same
/// cycle is served where the loop shows, and both end.
#[tokio::test]
async fn icsr_a_two_tenant_cycle_ends_with_508() {
    slots();
    let (_b, app) = broker(None, Vec::new()).await;
    grant(&app, "b", "a", json!({"access": "read-write"})).await;
    grant(&app, "a", "b", json!({"access": "read-write"})).await;
    let redirect = json!({"mode": "redirect", "operations": ["redirectionOps"],
                          "information": [{"entities": [{"type": "Car", "idPattern": "urn:ngsi-ld:Car:.*"}],
                                           "propertyNames": ["speed"]}]});
    for (tenant, id, target) in [("a", "ab", "b"), ("b", "ba", "a")] {
        let r = register(&app, tenant, id, Some(target), redirect.clone()).await;
        assert_eq!(r.status, 201, "{}", r.body);
    }
    let write = send(
        &app,
        "DELETE",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:x",
        Some("a"),
        None,
        None,
    )
    .await;
    assert_eq!(write.status, 508, "{} {:?}", write.body, write.warnings);
    let read = cars(&app, "a", None).await;
    assert!(ids(&read.body).is_empty(), "{} {}", read.status, read.body);
}

/// Counts how many decisions in Tenants `t*` run at once; each takes a
/// while, so forwards that were not bounded would overlap.
#[derive(Default)]
struct Overlap {
    now: AtomicUsize,
    most: AtomicUsize,
}

impl PolicyEngine for Overlap {
    fn name(&self) -> &'static str {
        "overlap"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let counted = s.tenant.as_str().starts_with('t');
        Box::pin(async move {
            if counted {
                let now = self.now.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                self.most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                self.now.fetch_sub(1, Ordering::SeqCst);
            }
            Decision::Allow
        })
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// C5: one query in a fans out to four granted Tenants in process; at most
/// `SLOTS` of those forwards run at once, as over HTTP, and all four answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn icsr_in_process_forwards_count_against_the_federation_bounds() {
    slots();
    let engine = Arc::new(Overlap::default());
    let (_b, app) = broker(Some(engine.clone() as Arc<dyn PolicyEngine>), Vec::new()).await;
    for n in 1..=4 {
        let target = format!("t{n}");
        car(&app, &target, n).await;
        grant(&app, &target, "a", json!({})).await;
        let r = register(&app, "a", &format!("r{n}"), Some(&target), json!({})).await;
        assert_eq!(r.status, 201, "{}", r.body);
    }
    engine.most.store(0, Ordering::SeqCst);
    let read = cars(&app, "a", None).await;
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(ids(&read.body).len(), 4, "{}", read.body);
    assert!(!warned(&read.warnings, "199"), "{:?}", read.warnings);
    let most = engine.most.load(Ordering::SeqCst);
    assert!((1..=SLOTS).contains(&most), "{most} forwards at once");
}
