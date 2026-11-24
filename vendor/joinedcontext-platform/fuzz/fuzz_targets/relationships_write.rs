//! Any model schema and any write payload: the relationship checks, the claims, the delete rule
//! and every refusal's problem never panic, a refusal names a slot the model states, and a named
//! target is of the class its end points at (T-7805).
#![no_main]

use jc_core::{type_of_id, ProblemDetails};
use jc_relationships::{
    claimed_twice, claims_of, deleted_attribute, first_missing, objects, targets_of, unlink,
    Extent, RelationshipRules, Shape,
};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;
use std::collections::BTreeSet;

fuzz_target!(|data: &[u8]| {
    // Schema JSON, NUL, payload JSON, NUL, the path's entity id and attribute, one per line.
    let mut parts = data.splitn(3, |b| *b == 0);
    let (Some(schema), Some(payload)) = (parts.next(), parts.next()) else {
        return;
    };
    let path = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    let mut path = path.splitn(2, '\n');
    let addressed = path.next().filter(|s| !s.is_empty());
    let targeted = path.next().filter(|s| !s.is_empty());
    let Ok(schema) = serde_json::from_slice::<Value>(schema) else {
        return;
    };
    let payload = serde_json::from_slice::<Value>(payload).unwrap_or(Value::Null);
    let rules = RelationshipRules::from_schema(&schema);
    let _ = objects(&payload);
    for extent in [Extent::Whole, Extent::Partial] {
        let shape = Shape {
            addressed,
            targeted,
            extent,
        };
        match targets_of(&payload, shape, &rules) {
            Ok(targets) => {
                for (urn, named) in &targets {
                    let kind = type_of_id(urn).expect("a target is an NGSI-LD URN");
                    let local = |t: &str| t.rsplit(['/', '#', ':']).next().unwrap_or(t).to_owned();
                    assert_eq!(local(kind), local(&named.target));
                }
                if let Some(missing) = first_missing(&targets, &BTreeSet::new()) {
                    let _ = ProblemDetails::from(missing);
                }
                let claims = claims_of(&payload, shape, &rules);
                if let Some(twice) = claimed_twice(&claims) {
                    assert_eq!(twice.taken().rule, "target-taken");
                }
            }
            Err(violation) => {
                assert!(!rules.is_empty());
                let slot = violation.slot.clone();
                let problem = ProblemDetails::from(violation);
                assert_eq!(problem.status, 400);
                assert_eq!(problem.extensions["slot"], Value::String(slot));
            }
        }
    }
    if let Some(id) = addressed {
        let _ = deleted_attribute(id, targeted.unwrap_or_default(), &rules);
        for referencing in rules.referencing_id(id) {
            let _ = unlink(&payload, referencing.slot, referencing.end, id);
        }
    }
});
