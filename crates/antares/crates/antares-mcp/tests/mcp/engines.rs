// SPDX-License-Identifier: EUPL-1.2
//! The policy engines the gate tests ask.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Allows everything and counts each decision.
#[derive(Default)]
pub struct Counting(pub AtomicUsize);

impl Counting {
    /// The decisions so far.
    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// Denies clause 5.7.5 (Retrieve Entity Types), allows the rest.
pub struct NoTypes;

/// Denies a query (clause 5.7.2) that names a type, so the probe, which
/// names nothing, passes and the call is refused at its gate.
pub struct NoNamedTypes;

/// Denies the information of a named type (clause 5.7.7), so the probe passes and
/// `get_type` and `describe_type` are refused at their gate.
pub struct NoTypeInfo;

/// Denies Create Subscription (clause 5.8.1): a stream watches nothing.
pub struct NoSubscriptions;

/// Denies every Delete Entity (clause 5.6.6): the probe hides the tool.
pub struct NoDeletes;

/// Denies Delete Entity (clause 5.6.6) of `urn:ngsi-ld:Car:w1`, so the
/// probe, which names no Entity, passes and the confirmed call is refused
/// at its own gate.
pub struct KeepW1;

fn notify() -> NotifyDecision {
    NotifyDecision::Deliver
}

impl PolicyEngine for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}

impl PolicyEngine for NoTypes {
    fn name(&self) -> &'static str {
        "no-types"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause == "5.7.5" {
            Decision::Deny("no".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}

impl PolicyEngine for NoNamedTypes {
    fn name(&self) -> &'static str {
        "no-named-types"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause != "5.7.2" || op.types.is_empty() {
            Decision::Allow
        } else {
            Decision::Deny("denied: speed<80 on secret".into())
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}

impl PolicyEngine for NoTypeInfo {
    fn name(&self) -> &'static str {
        "no-type-info"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause != "5.7.7" || op.types.is_empty() {
            Decision::Allow
        } else {
            Decision::Deny("denied: the type is not shared".into())
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}

impl PolicyEngine for KeepW1 {
    fn name(&self) -> &'static str {
        "keep-w1"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause == "5.6.6" && op.ids.contains(&"urn:ngsi-ld:Car:w1") {
            Decision::Deny("kept".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}

impl PolicyEngine for NoDeletes {
    fn name(&self) -> &'static str {
        "no-deletes"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause == "5.6.6" {
            Decision::Deny("no".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}

impl PolicyEngine for NoSubscriptions {
    fn name(&self) -> &'static str {
        "no-subscriptions"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause == "5.8.1" {
            Decision::Deny("no".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        notify()
    }
}
