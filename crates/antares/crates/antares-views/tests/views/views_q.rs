// SPDX-License-Identifier: EUPL-1.2
//! ADR-0023 rule 2 on every query path of the view root
//! (surfaces-test-plan section 6, V1, V2, V4, V9): a hidden Attribute does
//! not exist for the caller's `q`, its `orderBy` or a linked path.

use crate::common::{fixture, get, send, Answer, CARS, HISTORY};
use axum::http::{Method, StatusCode};
use serde_json::json;

fn enc(q: &str) -> String {
    q.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn sorted(a: &Answer) -> Vec<String> {
    let mut ids = a.ids();
    ids.sort();
    ids
}

/// What `q` selects through `cars` on each query path: the entity query,
/// the batch query and the temporal query.
async fn on_every_path(app: &axum::Router, q: &str) -> [Vec<String>; 3] {
    let query = get(app, &format!("{CARS}/entities?type=Car&q={}", enc(q))).await;
    let batch = send(
        app,
        Method::POST,
        &format!("{CARS}/entityOperations/query"),
        &[],
        Some(&json!({"type": "Query", "entities": [{"type": "Car"}], "q": q})),
    )
    .await;
    let temporal = get(
        app,
        &format!(
            "{CARS}/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z&timeproperty=modifiedAt&q={}",
            enc(q)
        ),
    )
    .await;
    for a in [&query, &batch, &temporal] {
        assert_eq!(a.status, StatusCode::OK, "{q}: {}", a.text);
    }
    [sorted(&query), sorted(&batch), sorted(&temporal)]
}

/// No answer depends on a hidden value: a term on `secret` with a value
/// below, at and above each stored one (1, 5, 9) selects what the same term
/// on an Attribute no Car has selects, on every query path, so nothing the
/// store holds under `secret` decides an answer. (The store's own query is
/// not observable from a surface, module rule M2; the AST rewrite itself is
/// `policy::tests::a_hidden_term_is_rewritten_and_the_engines_own_q_is_not`.)
#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_term_never_reaches_the_store() -> Result<(), String> {
    let f = fixture().await?;
    for v in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
        for op in ["==", "!=", ">", "<"] {
            let q = format!("secret{op}{v}");
            let hidden = on_every_path(&f.app, &q).await;
            let absent = on_every_path(&f.app, &format!("nothere{op}{v}")).await;
            assert_eq!(hidden, absent, "q={q}");
        }
    }
    let none = on_every_path(&f.app, "secret==5").await;
    assert!(none.iter().all(Vec::is_empty), "{none:?}");
    Ok(())
}

/// 7.6.2.2 puts an absent Attribute last, so ordering by a hidden one is
/// the order of an Attribute nobody has, in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn order_by_a_hidden_attribute_orders_as_absent() -> Result<(), String> {
    let f = fixture().await?;
    for dir in ["", ";desc", ";asc"] {
        let hidden = get(
            &f.app,
            &format!("{CARS}/entities?type=Car&orderBy=secret{}", enc(dir)),
        )
        .await;
        let absent = get(
            &f.app,
            &format!("{CARS}/entities?type=Car&orderBy=nothere{}", enc(dir)),
        )
        .await;
        assert_eq!(hidden.status, StatusCode::OK, "{}", hidden.text);
        assert_eq!(hidden.ids(), absent.ids(), "orderBy=secret{dir}");
    }
    let by_speed = get(
        &f.app,
        &format!("{CARS}/entities?type=Car&orderBy=speed{}", enc(";desc")),
    )
    .await;
    assert_eq!(
        by_speed.ids(),
        ["urn:ngsi-ld:Car:2", "urn:ngsi-ld:Car:1"],
        "a visible key still orders"
    );
    Ok(())
}

/// A linked path (4.9 `LinkedEntityRelation`) reads the target as the view
/// shows it: an out-of-view target is a dangling one, and a hidden
/// Attribute of an in-view target is absent.
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_q_path_does_not_cross_into_an_out_of_view_entity() -> Result<(), String> {
    let f = fixture().await?;
    let ids = |q: &str| {
        let app = f.app.clone();
        let q = q.to_owned();
        async move {
            sorted(
                &get(
                    &app,
                    &format!(
                        "{CARS}/entities?type=Car&join=inline&joinLevel=1&q={}",
                        enc(&q)
                    ),
                )
                .await,
            )
        }
    };
    assert_eq!(
        ids("owner{name}==\"Pat\"").await,
        ["urn:ngsi-ld:Car:1"],
        "an in-view target is read"
    );
    assert!(
        ids("driver{name}==\"Acme\"").await.is_empty(),
        "the out-of-view Org was read"
    );
    assert_eq!(
        ids("!driver{name}").await,
        ids("!nolink{name}").await,
        "out of view reads as dangling"
    );
    assert!(
        ids("owner{salary}>0").await.is_empty(),
        "the owner's hidden salary was read"
    );
    assert_eq!(ids("!owner{salary}").await, ids("!owner{nothere}").await);
    Ok(())
}

/// `MAX_Q_NODES` (512) counts the caller's `q` alone (threat V23): the
/// longest chain the NGSI-LD root accepts is accepted through a view
/// whatever the view's own predicate holds, and one term more is refused
/// alike, so the cap tells a caller nothing about the predicate's size.
#[tokio::test(flavor = "multi_thread")]
async fn the_q_node_cap_counts_the_callers_q_alone() -> Result<(), String> {
    const MAX_Q_NODES: usize = 512;
    let f = fixture().await?;
    let chain = |k: usize| vec!["a==1"; k].join("|");
    for (terms, ok) in [(MAX_Q_NODES - 1, true), (MAX_Q_NODES, false)] {
        let q = enc(&chain(terms));
        let direct = send(
            &f.app,
            Method::GET,
            &format!("/ngsi-ld/v1/entities?type=Car&q={q}"),
            &[("NGSILD-Tenant", "city")],
            None,
        )
        .await;
        assert_eq!(
            direct.status.is_success(),
            ok,
            "{terms} terms at the root: {}",
            direct.text
        );
        for root in [CARS, HISTORY] {
            let through = get(&f.app, &format!("{root}/entities?type=Car&q={q}")).await;
            assert_eq!(
                through.status, direct.status,
                "{terms} terms through {root}: {}",
                through.text
            );
            assert_eq!(
                through.json.get("title"),
                direct.json.get("title"),
                "{}",
                through.text
            );
        }
    }
    Ok(())
}
