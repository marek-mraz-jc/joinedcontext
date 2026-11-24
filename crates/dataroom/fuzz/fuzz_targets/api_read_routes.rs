//! Input: one byte that picks a read route, then a line of text and the rest: an OData path and
//! query (`/odata/v4/{text}`, `$filter`, `$select`, keys), a feed query with its `Accept` and
//! `Accept-Profile` headers (`/catalog.ttl?page=...&_profile=...`), a profile feed path, or the
//! portal search query. An anonymous read ends in an answer or a refusal, never a panic or a 500.
#![no_main]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use dataroom_api::{router, DataroomState};
use dataroom_core::{Catalogue, CatalogueId, OrgId, PolicyAuthorizer, RequestCtx, Store, Subject};
use dataroom_store_memory::MemoryStore;
use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use uuid::Uuid;

const HOST: &str = "data.city.test";

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    })
}

/// A catalogue with one public dataset, so a read has a row to filter, page and render.
async fn state() -> DataroomState {
    let org = OrgId::from_uuid(Uuid::from_u128(1));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(10));
    let store = Arc::new(MemoryStore::new());
    let catalogue: Catalogue = serde_json::from_value(serde_json::json!({
        "orgId": org, "id": cat, "name": "city", "host": HOST,
        "title": {"sk": "Mesto"}, "description": {}, "defaultLanguage": "sk",
        "languages": ["sk", "en"], "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-01T10:00:00Z",
    }))
    .expect("catalogue");
    store.upsert_catalogue(catalogue).await.expect("upsert");
    let ctx = RequestCtx::new(
        org,
        cat,
        Subject::authenticated("seed", vec![org], Vec::new()),
    );
    let dataset = serde_json::from_value(serde_json::json!({
        "orgId": org, "catalogueId": cat, "id": "pkg-air", "name": "air",
        "title": {"sk": "Ovzdušie"}, "description": {"sk": "Merania"}, "publisherId": "city",
        "visibility": "public", "state": "active", "distributions": [],
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    }))
    .expect("dataset");
    store.upsert_dataset(&ctx, dataset).await.expect("dataset");
    DataroomState::new(store, Arc::new(PolicyAuthorizer::default()), org, cat)
}

fuzz_target!(|data: &[u8]| {
    let Some((pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let (first, second) = text.split_once('\n').unwrap_or((text, ""));
    let uri = match pick % 4 {
        0 => format!("/odata/v4/{first}"),
        1 => format!("/catalog.ttl?{first}"),
        2 => format!("/profiles/{first}"),
        _ => format!("/api/3/action/package_search?{first}"),
    };
    let mut req = Request::get(&uri).header(header::HOST, HOST);
    if pick % 4 == 1 || pick % 4 == 2 {
        let (accept, profile) = second.split_once('\n').unwrap_or((second, ""));
        req = req
            .header(header::ACCEPT, accept)
            .header("accept-profile", profile);
    }
    let Ok(req) = req.body(Body::empty()) else {
        return;
    };
    runtime().block_on(async {
        let resp = router(state().await)
            .oneshot(req)
            .await
            .expect("infallible");
        let status = resp.status();
        assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
        axum::body::to_bytes(resp.into_body(), 64 << 20)
            .await
            .expect("body");
    });
});
