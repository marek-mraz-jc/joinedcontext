// SPDX-License-Identifier: EUPL-1.2
//! The state's maintenance and @context paths over a store that misbehaves
//! one call at a time: a purge stops at the first read it cannot make and
//! says so, a scan over a driver that cannot page is cut here, and the
//! @context write-through never overwrites a Hosted row nor fails a fetch
//! when the store refuses the write.
#![cfg(all(feature = "test-kit", feature = "views"))]

use antares_api::AppState;
use antares_model::TenantId;
use antares_sql::store::any::AnyStore;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn mem() -> Arc<AnyStore> {
    Arc::new(AnyStore::Mem(Default::default()))
}

fn faulting_on(tenant: &'static str) -> AppState {
    let store = Arc::new(crate::common::Double::faulting(mem(), move |m, t| {
        m == "list_page" && t.is_some_and(|t| t.as_str() == tenant)
    }));
    let temporal = mem();
    AppState::with_drivers("me".into(), store, temporal, "memory")
}

/// The purge walks the grants, then the views; a read it cannot make ends
/// it with the store's error, and what it had already removed stays removed.
#[tokio::test]
async fn a_purge_stops_at_the_walk_it_cannot_read() {
    let t = TenantId::try_from("purged".to_owned()).expect("tenant");

    let st = faulting_on("antares-grants");
    let err = st
        .purge_tenant_documents(&t)
        .await
        .expect_err("the grants walk is refused");
    assert!(err.contains("list_page refused"), "{err}");

    let st = faulting_on("antares-views");
    st.put_grant_doc(
        "purged~other",
        json!({"target": "purged", "source": "other"}),
    )
    .await
    .expect("grant stored");
    st.put_grant_doc("a~b", json!({"target": "a", "source": "b"}))
        .await
        .expect("grant stored");
    st.put_grant_doc("loose", json!({"target": "purged"}))
        .await
        .expect("grant stored");
    let err = st
        .purge_tenant_documents(&t)
        .await
        .expect_err("the views walk is refused");
    assert!(err.contains("list_page refused"), "{err}");
    assert_eq!(
        st.grant_doc("purged~other").await,
        Ok(None),
        "removed first"
    );
    assert!(
        st.grant_doc("a~b").await.expect("read").is_some(),
        "another Tenant's grant stays"
    );
    assert!(
        st.grant_doc("loose").await.expect("read").is_some(),
        "a grant naming no source is no party to the Tenant"
    );
}

/// A driver that answers every match unpaged: the page is cut here, and
/// it is the page the paging store answers.
#[tokio::test]
async fn a_scan_over_a_driver_that_cannot_page_is_cut_here() {
    let inner = mem();
    let plain = AppState::with_store("me".into(), inner.clone(), "memory");
    let t = TenantId::default();
    for i in 0..4 {
        let body = json!({"id": format!("urn:ngsi-ld:T:{i}"), "type": "T",
                          "speed": {"type": "Property", "value": i}})
        .to_string();
        let resp = antares_api::router(plain.clone())
            .oneshot(
                axum::http::Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(axum::body::Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
    }
    let unpaging = AppState::with_drivers(
        "me".into(),
        Arc::new(crate::common::Double::unpaging(inner.clone())),
        inner,
        "memory",
    );
    let (types, attrs) = ([format!("{DC}T")], [format!("{DC}speed")]);
    let paged = plain
        .scan_entities(&t, &types, &attrs, 1, 2)
        .await
        .expect("paged scan");
    let cut = unpaging
        .scan_entities(&t, &types, &attrs, 1, 2)
        .await
        .expect("cut scan");
    assert_eq!(paged.len(), 2, "{paged:?}");
    assert_eq!(cut, paged);
}

/// Serves `body` on every GET and counts the fetches.
fn ctx_server(path: &str, body: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let fetches = Arc::new(AtomicUsize::new(0));
    let n = fetches.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            n.fetch_add(1, Ordering::SeqCst);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ld+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            use std::io::{Read, Write};
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(resp.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}{path}"), fetches)
}

fn cached_id(url: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes()).to_string()
}

/// 5.13.1: a broker-local URL is a Hosted row, never a Cached copy. A
/// lookup that missed the row (here: its store read failed) fetches the
/// URL, and the write-through, finding the row then, writes no Cached copy
/// over it.
#[tokio::test(flavor = "multi_thread")]
async fn a_fetched_hosted_url_writes_no_cached_row() {
    antares_jsonld::allow_private_egress(true);
    let seg = "hosted-ctx-1";
    let (url, fetches) = ctx_server(
        &format!("/ngsi-ld/v1/jsonldContexts/{seg}"),
        r#"{"@context":{"other":"http://example.org/other"}}"#,
    );
    let t = TenantId::try_from("hosting".to_owned()).expect("tenant");
    let inner = mem();
    let hosted = json!({"url": url, "localId": seg, "kind": "Hosted", "owner": "hosting",
                        "createdAt": "2026-01-01T00:00:00.000Z",
                        "numberOfHits": 0, "lastUsage": null,
                        "body": {"@context": {"mine": "http://example.org/mine"}}});
    inner
        .context_put(Some(&t), seg, hosted.clone())
        .await
        .expect("hosted row");
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    let store = Arc::new(crate::common::Double::faulting(
        inner.clone(),
        move |m, _| m == "context_get" && seen.fetch_add(1, Ordering::SeqCst) == 0,
    ));
    let st = AppState::with_drivers("me".into(), store, inner.clone(), "memory");
    let ctx = st
        .loader
        .resolve_for(&t, &json!(url))
        .await
        .expect("resolved");
    assert_eq!(ctx.expand_key("other"), "http://example.org/other");
    assert_eq!(fetches.load(Ordering::SeqCst), 1, "the lookup missed");
    let row = inner
        .context_get(None, &cached_id(&url))
        .await
        .expect("read");
    assert!(row.is_none(), "no Cached row for a hosted URL: {row:?}");
    let kept = inner.context_get(Some(&t), seg).await.expect("read");
    assert_eq!(
        kept.as_ref().map(|r| &r["body"]),
        Some(&hosted["body"]),
        "{kept:?}"
    );
}

/// A store that refuses the write-through does not fail the fetch: the
/// @context resolves from the warm copy, with no second fetch.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_write_through_still_serves_the_fetched_context() {
    antares_jsonld::allow_private_egress(true);
    let (url, fetches) = ctx_server(
        "/refused-ctx.jsonld",
        r#"{"@context":{"refusedTerm":"http://example.org/refusedTerm"}}"#,
    );
    let pool = antares_sql::sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(300))
        .connect_lazy("postgres://antares@127.0.0.1:1/antares")
        .expect("a lazy pool parses");
    let store = AnyStore::Pg(antares_sql::store::any::PgBackend::new(pool));
    let st = AppState::with_store("me".into(), Arc::new(store), "postgres");
    st.loader.refetch(&url).await.expect("fetched");
    let ctx = st.loader.resolve(&json!(url)).await.expect("resolves");
    assert_eq!(
        ctx.expand_key("refusedTerm"),
        "http://example.org/refusedTerm"
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 1, "served warm");
}
