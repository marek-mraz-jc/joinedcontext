// SPDX-License-Identifier: EUPL-1.2
//! T-6055: a Query Entities answer of one 1000-Entity page holds each Entity
//! as written (~0.25 KB) until the body is read, not the page as stored
//! `Value` trees (~4.7 KB an Entity). The heavy OData kinds read such a page
//! per request, 30 at once, and held ~9 MB each.
//!
//! The heap is read from jemalloc's per-thread counters (the allocator the
//! broker ships), on a current-thread runtime, so the request's allocations
//! are this thread's.
#![cfg(not(target_env = "msvc"))]

use antares_api::AppState;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const PAGE: usize = 1000;

/// Bytes this thread holds: allocated minus freed since it started.
fn live() -> u64 {
    let allocated = tikv_jemalloc_ctl::thread::allocatedp::mib()
        .and_then(|m| m.read())
        .map_or(0, |p| p.get());
    let freed = tikv_jemalloc_ctl::thread::deallocatedp::mib()
        .and_then(|m| m.read())
        .map_or(0, |p| p.get());
    allocated.saturating_sub(freed)
}

/// The shape dev/perf/k6-surfaces.js seeds for the heavy load.
fn entity(n: usize) -> serde_json::Value {
    serde_json::json!({
        "id": format!("urn:ngsi-ld:Load:{n}"), "type": "Load",
        "name": {"type": "Property", "value": format!("Load {n}")},
        "level": {"type": "Property", "value": n % 100},
        "category": {"type": "Property", "value": format!("c{}", n % 8)},
        "location": {"type": "GeoProperty",
            "value": {"type": "Point", "coordinates": [19.0, 48.5]}},
    })
}

#[tokio::test(flavor = "current_thread")]
async fn a_1000_entity_page_is_held_as_written_until_it_is_read() {
    let st = AppState::new("t6055".into());
    let store = st.store.clone();
    let app = antares_api::router(st);
    let body = serde_json::Value::Array((0..PAGE).map(entity).collect()).to_string();
    let r = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/upsert")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("upsert");
    assert!(r.status().is_success(), "{}", r.status());

    // the page as `Value` trees, as the store read held it before
    let before = live();
    let docs = store
        .list(
            &antares_model::TenantId::default(),
            antares_store::Kind::Entity,
        )
        .await
        .expect("list");
    let as_values = live().saturating_sub(before);
    assert_eq!(docs.len(), PAGE);
    drop(docs);

    // a plain list, and an ordered one (sorted on trimmed documents, its
    // page read whole by id)
    for query in ["", "&orderBy=level"] {
        let (held, answer) = read(&app, query).await;
        // ~0.5 MB written against ~4.7 MB of trees: a quarter leaves room
        // for a wider shape
        assert!(
            held < as_values / 4,
            "{query}: the answer holds {held} bytes before it is read; the page as values is {as_values}"
        );
        // the same answer as the documents' path: `join` holds the page as
        // `Value`s, and these Entities link to nothing it would add
        let (_, joined) = read(&app, &format!("{query}&join=flat&joinLevel=1")).await;
        assert_eq!(answer, joined, "{query}");
    }
}

/// One page of the Load Entities with `query` appended: the bytes the
/// answer held before its body was read, and the body.
async fn read(app: &axum::Router, query: &str) -> (u64, axum::body::Bytes) {
    let before = live();
    let answer = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/ngsi-ld/v1/entities?type=Load&limit={PAGE}&count=true{query}"
            ))
            .header("Accept", "application/json")
            .body(Body::empty())
            .expect("request"),
        )
        .await
        .expect("query");
    let held = live().saturating_sub(before);
    assert_eq!(
        answer
            .headers()
            .get("NGSILD-Results-Count")
            .and_then(|v| v.to_str().ok()),
        Some("1000"),
        "{query}"
    );
    let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
        .await
        .expect("body");
    (held, body)
}
