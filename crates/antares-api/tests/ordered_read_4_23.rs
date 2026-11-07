// SPDX-License-Identifier: EUPL-1.2
//! T-4919: an ordered read the store decides sorts on what its `orderBy`
//! keys read and reads the page whole by id afterwards. Only Postgres
//! decides and projects, so every page it serves is held against the memory
//! store's, which sorts whole documents in the evaluator. Skips without
//! ANTARES_TEST_DATABASE_URL (the container recipe is in antares-sql/tests/pg.rs).
#![cfg(feature = "postgres")]

use antares_api::AppState;
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const VEHICLE: &str = "https://uri.etsi.org/ngsi-ld/default-context/Vehicle";

fn prop(v: Value) -> Value {
    json!([{"type": "Property", "value": v}])
}

/// Twenty Vehicles with a `level` that repeats, a `name` and a `note` the
/// sort never reads; `comma` adds one whose id holds a comma.
async fn seed(st: &AppState, tenant: &TenantId, comma: bool) {
    let d = |t: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{t}");
    let ids = (1..=20)
        .map(|i| format!("urn:ngsi-ld:Vehicle:o{i:02}"))
        .chain(comma.then(|| "urn:ngsi-ld:Vehicle:o21,x".to_owned()));
    for (i, id) in ids.enumerate() {
        let mut doc = json!({"id": id, "type": [VEHICLE]});
        doc[d("level")] = prop(json!(i % 6));
        doc[d("name")] = prop(json!(format!("vehicle {i}")));
        doc[d("note")] = prop(json!({"seats": i % 3}));
        st.store
            .create(tenant, Kind::Entity, &id, doc)
            .await
            .expect("seed");
    }
}

async fn pg_state(tenant_name: &str) -> Option<(AppState, TenantId)> {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = TenantId::new(tenant_name).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tenant)
        .await
        .expect("tenant row");
    let mut st = AppState::with_store(
        "antares-ordered".into(),
        Arc::new(antares_sql::store::any::AnyStore::Pg(
            antares_sql::store::any::PgBackend::new(pool),
        )),
        "postgres",
    );
    antares_api::wire(&mut st).await;
    for doc in st.store.list(&tenant, Kind::Entity).await.expect("list") {
        if let Some(id) = doc["id"].as_str() {
            st.store
                .delete(&tenant, Kind::Entity, id)
                .await
                .expect("clean");
        }
    }
    Some((st, tenant))
}

async fn get(st: &AppState, tenant: &TenantId, path: &str) -> (StatusCode, Option<String>, Value) {
    let req = Request::builder()
        .uri(path)
        .header("NGSILD-Tenant", tenant.as_str())
        .body(Body::empty())
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let count = res
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        count,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn same_pages(tenant_name: &str, comma: bool) {
    let Some((pg, tenant)) = pg_state(tenant_name).await else {
        return;
    };
    seed(&pg, &tenant, comma).await;
    let mut mem = AppState::new("antares-ordered".into());
    antares_api::wire(&mut mem).await;
    seed(&mem, &tenant, comma).await;
    for order in [
        "level;desc",
        "level",
        "level;desc,name",
        "note[seats],level;desc",
        "id;desc",
    ] {
        for offset in [0, 3, 7, 18] {
            let path = format!(
                "/ngsi-ld/v1/entities?type=Vehicle&q=level%3E=1&orderBy={order}&limit=4&offset={offset}&count=true"
            );
            let (status, count, page) = get(&pg, &tenant, &path).await;
            assert_eq!(status, StatusCode::OK, "{path}: {page}");
            let want = get(&mem, &tenant, &path).await;
            assert_eq!((status, count, &page), (want.0, want.1, &want.2), "{path}");
            // the page is read whole: members the sort never reads are there
            if let Some(first) = page.get(0) {
                assert!(first.get("note").is_some(), "{path}: {first}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ordered_page_from_trimmed_documents_equals_the_evaluators() {
    same_pages("orderedtrim", false).await;
}

/// The page is read back by an `id` list, which a comma would split: an id
/// holding one reads the match set whole instead.
#[tokio::test(flavor = "multi_thread")]
async fn an_id_holding_a_comma_reads_the_match_set_whole() {
    same_pages("orderedcomma", true).await;
}
