// SPDX-License-Identifier: EUPL-1.2
//! The last read decisions of each thread, handed out again while the same
//! input comes back (T-6618, finding R1-008).
//!
//! A read is decided afresh into a new [`Filter`](antares_api::policy::Filter)
//! of cloned rules (about 4.6 µs for three grants), and the broker's memos
//! after the gate (`views::paired`, `repr::narrow_repr`) then compared it by
//! value. A hit here answers the same `Arc`, so those memos find their answer
//! by pointer and the rules are neither built nor compared.
//!
//! A slot is keyed by the whole input, so it never answers for another: the
//! Tenant and its compiled document (the generation: a load swaps the `Arc`,
//! and the slot holds a `Weak` so the address cannot be reused), the
//! subject's carried headers (the caller and its view), every member of the
//! [`Operation`], and the span of time in which no policy of the table comes
//! into or out of force. A value unequal to itself (a NaN in `q`) misses.

use crate::document::Table;
use antares_api::policy::{Decision, Operation, StoredImage, Subject, FILTERABLE};
use antares_ql::geo::GeoQuery;
use antares_ql::QNode;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::cell::RefCell;
use std::sync::{Arc, Weak};

/// Decisions kept per thread: a worker thread serves a few callers in turn.
// ponytail: a short list scanned in order, the last used first; a per-Table map when a thread
// serves many more callers than this between two repeats.
const SLOTS: usize = 8;

/// One decision and everything it was decided on.
struct Slot {
    table: Weak<Table>,
    tenant: String,
    headers: Vec<(String, String)>,
    clause: &'static str,
    ids: Vec<String>,
    types: Vec<String>,
    attrs: Vec<String>,
    q: Option<QNode>,
    scope_q: Option<String>,
    geo: Option<GeoQuery>,
    body: Option<Value>,
    stored: Option<StoredImage>,
    /// No policy of the table comes into or out of force in `[from, until)`.
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    decision: Decision,
}

impl Slot {
    fn holds(
        &self,
        table: &Arc<Table>,
        subject: &Subject,
        op: &Operation<'_>,
        now: DateTime<Utc>,
    ) -> bool {
        std::ptr::eq(self.table.as_ptr(), Arc::as_ptr(table))
            && self.from.is_none_or(|from| now >= from)
            && self.until.is_none_or(|until| now < until)
            && self.clause == op.clause
            && self.tenant == subject.tenant.as_str()
            && self.headers == subject.headers
            && self
                .ids
                .iter()
                .map(String::as_str)
                .eq(op.ids.iter().copied())
            && self.types == op.types
            && self.attrs == op.attrs
            && self.q.as_ref() == op.q
            && self.scope_q.as_deref() == op.scope_q
            && self.geo.as_ref() == op.geo
            && self.body.as_ref() == op.body
            && self.stored.as_ref() == op.stored
    }
}

thread_local! {
    static LAST: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
}

/// `decide()` for a read under `table`, or the decision this thread made on
/// the same input. Anything but a [`FILTERABLE`] read, or a Tenant with no
/// document, is decided afresh every time.
pub(crate) fn decided(
    table: Option<&Arc<Table>>,
    subject: &Subject,
    op: &Operation<'_>,
    now: DateTime<Utc>,
    decide: impl FnOnce() -> Decision,
) -> Decision {
    let Some(table) = table.filter(|_| FILTERABLE.contains(&op.clause)) else {
        return decide();
    };
    // a hit moves to the front, so the reads a thread repeats stay kept
    let hit = LAST.with_borrow_mut(|last| {
        let at = last.iter().position(|s| s.holds(table, subject, op, now))?;
        last.get_mut(..=at)?.rotate_right(1);
        last.first().map(|s| s.decision.clone())
    });
    if let Some(hit) = hit {
        return hit;
    }
    let decision = decide();
    let (from, until) = steady(table.through(subject.view()), now);
    let slot = Slot {
        table: Arc::downgrade(table),
        tenant: subject.tenant.as_str().to_owned(),
        headers: subject.headers.clone(),
        clause: op.clause,
        ids: op.ids.iter().map(|&id| id.to_owned()).collect(),
        types: op.types.to_vec(),
        attrs: op.attrs.to_vec(),
        q: op.q.cloned(),
        scope_q: op.scope_q.map(str::to_owned),
        geo: op.geo.cloned(),
        body: op.body.cloned(),
        stored: op.stored.cloned(),
        from,
        until,
        decision: decision.clone(),
    };
    LAST.with_borrow_mut(|last| {
        last.truncate(SLOTS.saturating_sub(1));
        last.insert(0, slot);
    });
    decision
}

/// The span around `now` in which no policy of `table` comes into or out of
/// force: the latest validity bound at or before `now` and the first after.
fn steady(table: &Table, now: DateTime<Utc>) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let bounds = table
        .policies
        .iter()
        .flat_map(|p| [p.validity.0, p.validity.1])
        .flatten();
    bounds.fold((None, None), |(from, until), b| {
        if b <= now {
            (from.max(Some(b)), until)
        } else {
            (
                from,
                until.map_or(Some(b), |u: DateTime<Utc>| Some(u.min(b))),
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::compile;
    use chrono::Duration;
    use serde_json::json;
    use std::cell::Cell;

    const CAR: &str = "https://uri.etsi.org/ngsi-ld/default-context/Car";

    fn table(policies: &Value) -> Result<Arc<Table>, String> {
        compile(&json!({ "policies": policies })).map(Arc::new)
    }

    fn grant(group: &str) -> Value {
        json!({"id": format!("g-{group}"), "assignee": {"kind": "group", "id": group},
               "operations": ["5.7.2", "5.6.3"], "q": "speed<20",
               "information": [{"entities": [{"type": "Car"}]}]})
    }

    fn subject(tenant: &str, group: &str) -> Result<Subject, String> {
        Ok(Subject {
            tenant: antares_model::TenantId::new(tenant).map_err(|e| e.to_string())?,
            headers: vec![
                ("x-jc-subject".into(), format!("{group}@example.org")),
                ("x-jc-groups".into(), group.into()),
            ],
        })
    }

    /// `decided` with the engine's own decision, counting how often it ran.
    fn ask(
        t: &Arc<Table>,
        s: &Subject,
        op: &Operation<'_>,
        now: DateTime<Utc>,
        runs: &Cell<u32>,
    ) -> Decision {
        decided(Some(t), s, op, now, || {
            runs.set(runs.get().saturating_add(1));
            crate::decide::decide(
                Some(t.through(s.view())),
                &crate::subject::Caller::of(s),
                op,
                now,
            )
        })
    }

    fn filter(d: &Decision) -> Result<&Arc<antares_api::policy::Filter>, String> {
        match d {
            Decision::Filter(f) => Ok(f),
            other => Err(format!("not a filter: {other:?}")),
        }
    }

    /// R1-008: the same read twice is one decision and one `Arc`.
    #[test]
    fn the_same_read_is_answered_with_the_same_filter() -> Result<(), String> {
        let t = table(&json!([grant("city")]))?;
        let s = subject("t6618-same", "city")?;
        let types = [CAR.to_owned()];
        let op = Operation {
            types: &types,
            ..Operation::new("5.7.2")
        };
        let (now, runs) = (Utc::now(), Cell::new(0));
        let first = ask(&t, &s, &op, now, &runs);
        let again = ask(&t, &s, &op, now, &runs);
        assert!(Arc::ptr_eq(filter(&first)?, filter(&again)?));
        assert_eq!(runs.get(), 1);
        Ok(())
    }

    fn near() -> Result<GeoQuery, String> {
        let area: std::collections::HashMap<String, String> = [
            ("georel", "near;maxDistance==10"),
            ("geometry", "Point"),
            ("coordinates", "[1,2]"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        GeoQuery::from_params(&area)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no geoquery".to_owned())
    }

    /// Each member of the input is a key: another caller, Tenant, clause or
    /// Operation member is decided afresh (and its repeat kept), and a caller
    /// without the grant is refused although the last answer allowed.
    #[test]
    fn another_input_is_decided_afresh() -> Result<(), String> {
        let t = table(&json!([grant("city")]))?;
        let (types, bus) = ([CAR.to_owned()], [CAR.replace("Car", "Bus")]);
        let op = || Operation {
            types: &types,
            ..Operation::new("5.7.2")
        };
        let (now, runs) = (Utc::now(), Cell::new(0));
        let city = subject("t6618-other", "city")?;
        let (stranger, elsewhere) = (subject("t6618-other", "x")?, subject("t6618-else", "city")?);
        let q = antares_ql::parse_q("speed>1").map_err(|e| e.to_string())?;
        let geo = near()?;
        let (body, stored) = (json!({"q": "x"}), StoredImage::default());
        let (ids, attrs) = (["urn:ngsi-ld:Car:1"], [CAR.replace("Car", "speed")]);
        let asked: [(&Subject, Operation<'_>); 11] = [
            (
                &city,
                Operation {
                    clause: "5.7.1",
                    ..op()
                },
            ),
            (&stranger, op()),
            (&elsewhere, op()),
            (
                &city,
                Operation {
                    types: &bus,
                    ..op()
                },
            ),
            (
                &city,
                Operation {
                    q: Some(&q),
                    ..op()
                },
            ),
            (
                &city,
                Operation {
                    attrs: &attrs,
                    ..op()
                },
            ),
            (&city, Operation { ids: &ids, ..op() }),
            (
                &city,
                Operation {
                    scope_q: Some("/a"),
                    ..op()
                },
            ),
            (
                &city,
                Operation {
                    geo: Some(&geo),
                    ..op()
                },
            ),
            (
                &city,
                Operation {
                    body: Some(&body),
                    ..op()
                },
            ),
            (
                &city,
                Operation {
                    stored: Some(&stored),
                    ..op()
                },
            ),
        ];
        // each right after the read it differs from by one member, so that
        // read's slot is there to answer wrongly
        ask(&t, &city, &op(), now, &runs);
        for (n, (s, other)) in (2u32..).zip(&asked) {
            ask(&t, &city, &op(), now, &runs);
            let first = ask(&t, s, other, now, &runs);
            assert_eq!(ask(&t, s, other, now, &runs), first);
            assert_eq!(runs.get(), n, "{n}: {other:?}");
        }
        // eight are kept: the stranger's read was dropped, and is decided
        // (and refused) again
        assert!(matches!(
            ask(&t, &stranger, &op(), now, &runs),
            Decision::Deny(_)
        ));
        assert_eq!(runs.get(), 13);
        // a write is never kept
        let write = Operation {
            types: &types,
            ..Operation::new("5.6.3")
        };
        ask(&t, &city, &write, now, &runs);
        ask(&t, &city, &write, now, &runs);
        assert_eq!(runs.get(), 15);
        Ok(())
    }

    /// A load is a new generation: a revoked grant refuses at once.
    #[test]
    fn a_reloaded_document_is_decided_afresh() -> Result<(), String> {
        let s = subject("t6618-reload", "city")?;
        let types = [CAR.to_owned()];
        let op = Operation {
            types: &types,
            ..Operation::new("5.7.2")
        };
        let (now, runs) = (Utc::now(), Cell::new(0));
        let granted = table(&json!([grant("city")]))?;
        filter(&ask(&granted, &s, &op, now, &runs))?;
        let revoked = table(&json!([grant("other")]))?;
        assert!(matches!(
            ask(&revoked, &s, &op, now, &runs),
            Decision::Deny(_)
        ));
        assert_eq!(runs.get(), 2);
        Ok(())
    }

    /// A grant whose validity ends refuses from that instant, kept answer or not.
    #[test]
    fn a_grant_that_expires_is_decided_afresh() -> Result<(), String> {
        let now = Utc::now();
        let end = now + Duration::seconds(30);
        let mut timed = grant("city");
        if let Some(o) = timed.as_object_mut() {
            o.insert("validity".into(), json!({"to": end.to_rfc3339()}));
        }
        let t = table(&json!([timed]))?;
        let s = subject("t6618-expiry", "city")?;
        let types = [CAR.to_owned()];
        let op = Operation {
            types: &types,
            ..Operation::new("5.7.2")
        };
        let runs = Cell::new(0);
        filter(&ask(&t, &s, &op, now, &runs))?;
        filter(&ask(&t, &s, &op, now + Duration::seconds(29), &runs))?;
        assert_eq!(runs.get(), 1);
        assert!(matches!(ask(&t, &s, &op, end, &runs), Decision::Deny(_)));
        assert_eq!(runs.get(), 2);
        // a refusal kept from `end` on never answers for a time before it
        filter(&ask(&t, &s, &op, now, &runs))?;
        assert_eq!(runs.get(), 2);
        assert_eq!(steady(&t, now), (None, Some(end)));
        assert_eq!(steady(&t, end), (Some(end), None));
        Ok(())
    }
}
