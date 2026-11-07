// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rule 3, threat C4: a chain of internal forwards deeper than
//! `MAX_IN_PROCESS_CALL_DEPTH` (8) ends at the bound with code 199; the
//! forward past it is not made, in process or over HTTP. Its own binary:
//! the chain holds nine federation slots at once.

mod common;

use common::{broker, car, cars, grant, ids, register, warned};
use serde_json::json;

/// t0 registers t1, t1 registers t2, and so on to t9; t8 and t9 hold a Car.
/// A query in t0 reaches t8 (eight in-process calls deep) and not t9.
#[tokio::test]
async fn icsr_past_the_depth_bound_no_forward_is_made() {
    let (_b, app) = broker(None, Vec::new()).await;
    for n in 0..9u32 {
        let (source, target) = (format!("t{n}"), format!("t{}", n.saturating_add(1)));
        grant(&app, &target, &source, json!({})).await;
        let r = register(&app, &source, &format!("r{n}"), Some(&target), json!({})).await;
        assert_eq!(r.status, 201, "{}", r.body);
    }
    car(&app, "t8", 8).await;
    car(&app, "t9", 9).await;
    let read = cars(&app, "t0", None).await;
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(ids(&read.body), ["urn:ngsi-ld:Car:t8-8"], "{}", read.body);
    assert!(warned(&read.warnings, "199"), "{:?}", read.warnings);
}
