//! Input: one byte that picks the method and the caller (anonymous or an admin), then a CKAN
//! action name, a newline, and the rest: the JSON body of a POST or the query string of a GET, as
//! a CKAN client sends it to `/api/3/action/{action}`. Every request ends in a CKAN envelope with
//! a status, never a panic; an anonymous caller never writes.
#![no_main]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use dataroom_api::{router, DataroomState};
use dataroom_core::{
    Action, Catalogue, CatalogueId, GrantRule, OrgId, PolicyAuthorizer, Store, Subject,
};
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
    let authz = PolicyAuthorizer::new(vec![GrantRule {
        org_id: org,
        catalogue_id: None,
        principal: "admin".into(),
        actions: vec![Action::Admin],
        publisher_id: None,
    }]);
    DataroomState::new(store, Arc::new(authz), org, cat).with_subject_resolver(move |headers| {
        (headers.get(header::AUTHORIZATION)?.to_str().ok()? == "Bearer admin")
            .then(|| Subject::authenticated("admin", vec![org], Vec::new()))
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((pick, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let (action, payload) = text.split_once('\n').unwrap_or((text, ""));
    if action.is_empty()
        || !action
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return;
    }
    let post = pick & 1 == 0;
    let admin = pick & 2 == 2;
    let uri = if post {
        format!("/api/3/action/{action}")
    } else {
        format!("/api/3/action/{action}?{payload}")
    };
    let mut req = Request::builder()
        .method(if post { Method::POST } else { Method::GET })
        .uri(&uri)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/json");
    if admin {
        req = req.header(header::AUTHORIZATION, "Bearer admin");
    }
    let body = if post {
        Body::from(payload.to_owned())
    } else {
        Body::empty()
    };
    let Ok(req) = req.body(body) else {
        return;
    };
    runtime().block_on(async {
        let resp = router(state().await)
            .oneshot(req)
            .await
            .expect("infallible");
        let status = resp.status();
        assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20)
            .await
            .expect("body");
        if !admin && status.is_success() && post {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            // an anonymous POST that succeeded wrote nothing a reader could not
            let writes = ["_create", "_update", "_patch", "_delete", "_purge"];
            assert!(
                !writes.iter().any(|w| action.ends_with(w)),
                "anonymous {action} succeeded: {v}"
            );
        }
    });
});
