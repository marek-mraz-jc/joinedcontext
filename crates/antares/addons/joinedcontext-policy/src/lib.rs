// SPDX-License-Identifier: EUPL-1.2
//! The joinedcontext platform's policy engine (ADR-0020 addon, README):
//! the Tenant's Policy rules, loaded through `/q/policy/{tenant}`, decided
//! on the `Operation` and the `Subject` alone. No network, no store, no
//! token: the gateway verified the caller and forwards who it is.
//!
//! ```
//! use antares_api::policy::PolicyEngine;
//! use joinedcontext_policy::JoinedcontextPolicy;
//! use serde_json::json;
//!
//! let engine = JoinedcontextPolicy::new();
//! engine.load("city", &json!({"policies": [{
//!     "id": "open-data", "effect": "permit",
//!     "assignee": {"kind": "role", "id": "public"},
//!     "operations": ["5.7.1", "5.7.2"],
//!     "information": [{"entities": [{"type": "Parking"}]}]
//! }]}))?;
//! assert!(engine.table("city").is_some());
//! assert!(engine.load("city", &json!({"policies": [{"id": ""}]})).is_err());
//! assert!(engine.table("city").is_some(), "a refused document keeps the last one");
//! # Ok::<(), String>(())
//! ```

pub mod access;
pub mod decide;
pub mod document;
mod memo;
pub mod names;
pub mod odrl;
pub mod subject;
pub mod surface;
pub mod ucast;

use antares_api::policy::{
    Decision, DecisionFuture, DecisionsFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use document::Table;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard};
use subject::Caller;

/// The name the engine is selected by (`ANTARES_POLICY=joinedcontext`).
pub const POLICY_NAME: &str = "joinedcontext";

/// The engine: each Tenant's compiled document, swapped whole on load.
#[derive(Default)]
pub struct JoinedcontextPolicy {
    tables: RwLock<HashMap<String, Arc<Table>>>,
}

impl JoinedcontextPolicy {
    /// An engine with no document: it denies every platform caller.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The engine of this process: the one the broker decides with and the
    /// access surface reads, so the two answer from one table.
    #[must_use]
    pub fn process() -> Arc<Self> {
        static ENGINE: std::sync::OnceLock<Arc<JoinedcontextPolicy>> = std::sync::OnceLock::new();
        ENGINE.get_or_init(Arc::default).clone()
    }

    /// The compiled document of `tenant`.
    #[must_use]
    pub fn table(&self, tenant: &str) -> Option<Arc<Table>> {
        self.read().get(tenant).cloned()
    }

    // A writer holds the lock for one insert or remove of an `Arc`, which
    // leaves the map whole even if it panicked: a poisoned lock is read on.
    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, Arc<Table>>> {
        self.tables.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The types of the Entities a notification carries, expanded with `ctx`, the
    /// `@context` the data is compacted with: what the query it answers named (README 3.8).
    fn notified_types(notification: &Value, ctx: &antares_jsonld::Context) -> Vec<String> {
        let mut out: Vec<String> = notification
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("type"))
            .flat_map(|t| match t {
                Value::String(one) => vec![one.as_str()],
                Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            })
            .map(|t| ctx.expand_key(t))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn decide_now(&self, subject: &Subject, op: &Operation<'_>) -> Decision {
        let table = self.table(subject.tenant.as_str());
        let now = chrono::Utc::now();
        memo::decided(table.as_ref(), subject, op, now, || {
            decide::decide(
                table.as_deref().map(|t| t.through(subject.view())),
                &Caller::of(subject),
                op,
                now,
            )
        })
    }
}

impl PolicyEngine for JoinedcontextPolicy {
    fn name(&self) -> &str {
        POLICY_NAME
    }

    fn decide<'a>(&'a self, subject: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(self.decide_now(subject, op)))
    }

    fn offers_each<'a>(
        &'a self,
        subject: &'a Subject,
        ops: &'a [Operation<'a>],
    ) -> DecisionsFuture<'a> {
        let table = self.table(subject.tenant.as_str());
        let table = table.as_deref().map(|t| t.through(subject.view()));
        let caller = Caller::of(subject);
        let now = chrono::Utc::now();
        Box::pin(std::future::ready(
            ops.iter()
                .map(|op| decide::probe(table, &caller, op, now))
                .collect(),
        ))
    }

    /// A notification is the answer to a query by the Subscription's
    /// stored subject naming the types of the Entities it carries, decided
    /// again at delivery: a revoked grant stops the next one, and a grant
    /// that takes the queried types reads them (T-3784).
    fn pre_notify(
        &self,
        subject: &Subject,
        sub: &Value,
        notification: &mut Value,
    ) -> NotifyDecision {
        self.pre_notify_in(subject, sub, notification, document::context())
    }

    /// A subscriber's `@context` can alias a prohibited type's IRI with another short name,
    /// so the notified types are expanded with that context, never the core one
    /// (F-R2ANT1-002).
    fn pre_notify_in(
        &self,
        subject: &Subject,
        _sub: &Value,
        notification: &mut Value,
        ctx: &antares_jsonld::Context,
    ) -> NotifyDecision {
        let types = Self::notified_types(notification, ctx);
        let op = Operation {
            types: &types,
            ..Operation::new("5.7.2")
        };
        match self.decide_now(subject, &op) {
            Decision::Allow => NotifyDecision::Deliver,
            Decision::Filter(f) => NotifyDecision::Filter(Box::new(Arc::unwrap_or_clone(f))),
            Decision::Deny(_) | Decision::AllowIf(_) => NotifyDecision::Drop,
        }
    }

    fn loads_documents(&self) -> bool {
        true
    }

    fn load(&self, tenant: &str, document: &Value) -> Result<(), String> {
        let table = if document.is_null() {
            None
        } else {
            Some(Arc::new(document::compile(document)?))
        };
        let mut tables = self.tables.write().unwrap_or_else(PoisonError::into_inner);
        match table {
            Some(t) => tables.insert(tenant.to_owned(), t),
            None => tables.remove(tenant),
        };
        Ok(())
    }
}
