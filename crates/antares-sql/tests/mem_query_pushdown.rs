// SPDX-License-Identifier: EUPL-1.2
//! T-3556: the memory and file stores filter a query on the stored documents
//! and copy only what the answer holds. ids, types and attrs are exact there,
//! so with nothing else asked the outcome is `decided` and a page is cut in
//! id order, the store's default, with its total. A `q` is decided there
//! too when the request @context comes along and it has no linked term
//! (T-3572); a linked `q`, a `scopeQ` or a geo query leaves the caller the
//! arbiter over the type-narrowed rows.

use antares_model::TenantId;
use antares_sql::store::any::AnyStore;
use antares_sql::store::filter::{EntityFilter, Page};
use antares_sql::store::{Kind, Store};
use serde_json::{json, Value};

const T: &str = "https://example.org/T";
const U: &str = "https://example.org/U";
const SPEED: &str = "https://example.org/speed";

async fn seeded() -> (AnyStore, TenantId) {
    let s = AnyStore::Mem(Store::default());
    let t = TenantId::new("mempush").expect("tenant");
    let docs = [
        json!({"id": "urn:ngsi-ld:T:1", "type": [T], SPEED: [{"type": "Property", "value": 1}]}),
        json!({"id": "urn:ngsi-ld:T:2", "type": [T]}),
        json!({"id": "urn:ngsi-ld:T:3", "type": [T], SPEED: [{"type": "Property", "value": 3}]}),
        json!({"id": "urn:ngsi-ld:T:4", "type": [T], "expiresAt": "2000-01-01T00:00:00Z"}),
        json!({"id": "urn:ngsi-ld:U:1", "type": [U], SPEED: [{"type": "Property", "value": 9}]}),
    ];
    for d in docs {
        let id = d["id"].as_str().expect("id").to_owned();
        assert!(s.create(&t, Kind::Entity, &id, d).await.expect("create"));
    }
    (s, t)
}

fn ids(rows: &[Value]) -> Vec<&str> {
    rows.iter().filter_map(|r| r["id"].as_str()).collect()
}

#[tokio::test]
async fn a_type_query_is_decided_and_paged_in_id_order() {
    let (s, t) = seeded().await;
    let types = vec![vec![T.to_owned()]];
    let page = |offset, count| {
        Some(Page {
            offset,
            limit: 1,
            count,
        })
    };
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                types: Some(&types),
                page: page(1, true),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(out.decided && out.paged);
    assert_eq!(
        ids(&out.rows),
        ["urn:ngsi-ld:T:2"],
        "the second of T:1..T:3"
    );
    assert_eq!(out.total, Some(3), "the expired T:4 is not a match");

    // uncounted: the total says only whether a next page exists
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                types: Some(&types),
                page: page(0, false),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert_eq!(ids(&out.rows), ["urn:ngsi-ld:T:1"]);
    assert_eq!(out.total, Some(2), "offset 0 + 1 row + a next page");

    // off the end: no rows, and the count still holds
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                types: Some(&types),
                page: page(9, true),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(out.rows.is_empty());
    assert_eq!(out.total, Some(3));
}

#[tokio::test]
async fn ids_attrs_and_projection_apply_in_the_store() {
    let (s, t) = seeded().await;
    let want = [
        "urn:ngsi-ld:T:3",
        "urn:ngsi-ld:U:1",
        "urn:ngsi-ld:T:2",
        "urn:ngsi-ld:none",
    ];
    let attrs = vec![SPEED.to_owned()];
    let keep: Vec<String> = Vec::new();
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                ids: Some(&want),
                attrs: Some(&attrs),
                keep_attrs: Some(&keep),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(out.decided && !out.paged);
    assert_eq!(
        ids(&out.rows),
        ["urn:ngsi-ld:T:3", "urn:ngsi-ld:U:1"],
        "id order"
    );
    assert!(
        out.rows.iter().all(|r| r.get(SPEED).is_none()),
        "kept no attribute"
    );
    assert!(
        out.rows.iter().all(|r| r.get("type").is_some()),
        "the frame stays"
    );
}

#[tokio::test]
async fn a_q_leaves_the_caller_the_arbiter_over_the_narrowed_rows() {
    let (s, t) = seeded().await;
    let types = vec![vec![T.to_owned()]];
    let q = antares_ql::parse_q("speed>2").expect("q");
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                types: Some(&types),
                q: Some(&q),
                page: Some(Page {
                    offset: 0,
                    limit: 1,
                    count: true,
                }),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(!out.decided && !out.paged && out.total.is_none());
    assert_eq!(
        ids(&out.rows),
        ["urn:ngsi-ld:T:1", "urn:ngsi-ld:T:2", "urn:ngsi-ld:T:3"],
        "type-narrowed, unpaged, the expired one left out"
    );
}

/// T-3572: with the request @context the store runs the evaluator the API
/// runs (`antares_ql::eval::eval_q`) on the borrowed documents, so a `q`
/// without linked-entity terms is decided in the store and only the page is
/// copied. A linked term reads other Entities through the caller's narrowing
/// and stays with the caller.
#[tokio::test]
async fn a_q_with_the_context_is_decided_in_the_store() {
    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context";
    let s = AnyStore::Mem(Store::default());
    let t = TenantId::new("mempushq").expect("tenant");
    for (i, v) in [1, 50, 99].into_iter().enumerate() {
        let id = format!("urn:ngsi-ld:Vehicle:{i}");
        let doc = json!({"id": id, "type": [format!("{DC}/Vehicle")],
            format!("{DC}/speed"): [{"type": "Property", "value": v}]});
        assert!(s.create(&t, Kind::Entity, &id, doc).await.expect("create"));
    }
    let ctx = antares_jsonld::core_context();
    let types = vec![vec![format!("{DC}/Vehicle")]];
    let query = |q: &str| antares_ql::parse_q(q).expect("q");
    let page = Some(Page {
        offset: 0,
        limit: 1,
        count: true,
    });

    let q = query("speed>10");
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                types: Some(&types),
                q: Some(&q),
                ctx: Some(&ctx),
                page,
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(out.decided && out.paged, "a flat q is exact in the store");
    assert_eq!(
        ids(&out.rows),
        ["urn:ngsi-ld:Vehicle:1"],
        "the first match only"
    );
    assert_eq!(out.total, Some(2), "50 and 99 match");

    let q = query("driver{speed}>10");
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                types: Some(&types),
                q: Some(&q),
                ctx: Some(&ctx),
                page: Some(Page {
                    offset: 0,
                    limit: 1,
                    count: true,
                }),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(
        !out.decided && !out.paged,
        "a linked term stays with the caller"
    );
    assert_eq!(out.rows.len(), 3, "type-narrowed, unpaged");
}

/// 4.22 / F-ANT1-007: a decided `q` judges what a read may see. An Entity
/// whose only matching instance expired is no match and is not counted; one
/// with a live matching instance beside an expired one still is.
#[tokio::test]
async fn a_q_never_matches_an_expired_instance() {
    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context";
    let s = AnyStore::Mem(Store::default());
    let t = TenantId::new("mempushexp").expect("tenant");
    let gone = json!([{"type": "Property", "value": 20, "expiresAt": "2000-01-01T00:00:00Z"}]);
    let kept = json!([
        {"type": "Property", "value": 30, "expiresAt": "2000-01-01T00:00:00Z", "datasetId": "urn:d:1"},
        {"type": "Property", "value": 40}
    ]);
    for (id, speed) in [
        ("urn:ngsi-ld:Vehicle:gone", gone),
        ("urn:ngsi-ld:Vehicle:kept", kept),
    ] {
        let doc =
            json!({"id": id, "type": [format!("{DC}/Vehicle")], format!("{DC}/speed"): speed});
        assert!(s.create(&t, Kind::Entity, id, doc).await.expect("create"));
    }
    let ctx = antares_jsonld::core_context();
    for (term, want) in [
        ("speed>10", vec!["urn:ngsi-ld:Vehicle:kept"]),
        ("speed==30", vec![]),
        ("speed==20", vec![]),
    ] {
        let q = antares_ql::parse_q(term).expect("q");
        let out = s
            .query_entities(
                &t,
                &EntityFilter {
                    q: Some(&q),
                    ctx: Some(&ctx),
                    page: Some(Page {
                        offset: 0,
                        limit: 10,
                        count: true,
                    }),
                    ..Default::default()
                },
            )
            .await
            .expect("query");
        assert!(out.decided, "{term}");
        assert_eq!(ids(&out.rows), want, "{term}");
        assert_eq!(
            out.total,
            Some(i64::try_from(want.len()).expect("len")),
            "{term}"
        );
    }
}

/// T-5826: a rule's `idPatterns` are exact here too, each read whole; one
/// that does not compile matches nothing, as the rule's own check reads it.
#[tokio::test]
async fn id_patterns_are_decided_and_counted() {
    let (s, t) = seeded().await;
    let ps = ["urn:ngsi-ld:T:[13]".to_owned(), "(unclosed".to_owned()];
    let out = s
        .query_entities(
            &t,
            &EntityFilter {
                id_patterns: Some(&ps),
                page: Some(Page {
                    offset: 0,
                    limit: 1,
                    count: true,
                }),
                ..Default::default()
            },
        )
        .await
        .expect("query");
    assert!(out.decided);
    assert_eq!(ids(&out.rows), ["urn:ngsi-ld:T:1"]);
    assert_eq!(out.total, Some(2));
}
