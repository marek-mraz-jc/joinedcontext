// SPDX-License-Identifier: EUPL-1.2
//! ADR-0030: a Tenant exported and imported into an empty one keeps its
//! system attributes and its history, so the digests of both sides agree;
//! a cut stream, a foreign header or a non-empty target imports nothing,
//! and v1 refuses to export what it does not carry.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(
    st: &AppState,
    method: &str,
    path: &str,
    body: impl Into<String>,
    tenant: Option<&str>,
    json: bool,
) -> (StatusCode, Vec<u8>) {
    let body: String = body.into();
    let mut b = Request::builder().method(method).uri(path);
    if !body.is_empty() {
        b = b.header("Content-Length", body.len());
    }
    if let Some(t) = tenant {
        b = b.header("NGSILD-Tenant", t);
    }
    if json {
        b = b.header("Content-Type", "application/json");
    }
    let res = antares_api::router(st.clone())
        .oneshot(b.body(Body::from(body)).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, bytes.to_vec())
}

async fn ngsi(st: &AppState, method: &str, path: &str, body: Value, tenant: &str) -> StatusCode {
    call(st, method, path, body.to_string(), Some(tenant), true)
        .await
        .0
}

async fn read(st: &AppState, path: &str, tenant: Option<&str>) -> Value {
    let (s, b) = call(st, "GET", path, "", tenant, false).await;
    assert_eq!(s, StatusCode::OK, "{path}: {}", String::from_utf8_lossy(&b));
    serde_json::from_slice(&b).expect("JSON")
}

async fn state() -> AppState {
    let mut st = AppState::new("antares-test".into());
    antares_api::wire(&mut st).await;
    st
}

async fn seeded(st: &AppState, tenant: &str) {
    for n in 1..=3 {
        let room = json!({"id": format!("urn:ngsi-ld:Room:{n}"), "type": "Room",
            "temperature": {"type": "Property", "value": 20 + n, "observedAt": "2026-01-01T00:00:00Z"}});
        assert_eq!(
            ngsi(st, "POST", "/ngsi-ld/v1/entities", room, tenant).await,
            StatusCode::CREATED
        );
    }
    let later = json!({"temperature": {"type": "Property", "value": 30, "observedAt": "2026-01-02T00:00:00Z"}});
    assert_eq!(
        ngsi(
            st,
            "PATCH",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Room:1/attrs",
            later,
            tenant
        )
        .await,
        StatusCode::NO_CONTENT
    );
}

async fn export(st: &AppState, tenant: &str) -> String {
    let (s, b) = call(
        st,
        "GET",
        &format!("/q/tenants/{tenant}/export"),
        "",
        None,
        false,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    String::from_utf8(b).expect("utf-8")
}

async fn import(st: &AppState, tenant: &str, stream: String) -> (StatusCode, String) {
    let (s, b) = call(
        st,
        "POST",
        &format!("/q/tenants/{tenant}/import"),
        stream,
        None,
        false,
    )
    .await;
    (s, String::from_utf8_lossy(&b).into_owned())
}

/// Two brokers, one tenant name: the move ADR-N-046 §2.6 makes.
#[tokio::test]
async fn an_exported_tenant_imports_with_its_system_attributes_and_history() {
    let (source, target) = (state().await, state().await);
    seeded(&source, "air").await;
    let before = read(&source, "/q/tenants/air/digest", None).await;
    assert_eq!(before["entities"], 3);
    assert!(
        before["attrInstances"].as_u64().unwrap_or(0) >= 4,
        "{before}"
    );

    let stream = export(&source, "air").await;
    let lines: Vec<Value> = stream
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect();
    assert_eq!(lines[0], json!({"antaresExport": 1, "tenant": "air"}));
    assert_eq!(
        lines.last().expect("a trailer"),
        &json!({"end": true, "records": lines.len() - 2})
    );
    let kinds: Vec<&str> = lines[1..lines.len() - 1]
        .iter()
        .filter_map(|l| l["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        ["entity", "entity", "entity", "temporal", "temporal", "temporal"],
        "entities first"
    );

    let (status, why) = import(&target, "air", stream.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{why}");
    let after = read(&target, "/q/tenants/air/digest", None).await;
    assert_eq!(after, before, "count, history and hash read on both sides");
    for path in [
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Room:1",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Room:1?options=sysAttrs",
    ] {
        assert_eq!(
            read(&target, path, Some("air")).await,
            read(&source, path, Some("air")).await,
            "{path}"
        );
    }

    let (status, why) = import(&target, "air", stream).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a second import into a full tenant: {why}"
    );
}

#[tokio::test]
async fn a_cut_or_foreign_stream_imports_nothing() {
    let (source, target) = (state().await, state().await);
    seeded(&source, "air").await;
    let stream = export(&source, "air").await;
    let mut lines: Vec<&str> = stream.lines().collect();

    let without_trailer = lines[..lines.len() - 1].join("\n") + "\n";
    let (status, why) = import(&target, "air", without_trailer).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{why}");
    assert!(why.contains("trailer"), "{why}");

    let (status, why) = import(&target, "air", stream.trim_end().to_owned()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{why}");

    let foreign = stream.replacen("\"tenant\":\"air\"", "\"tenant\":\"water\"", 1);
    let (status, why) = import(&target, "air", foreign).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(why.contains("another tenant"), "{why}");

    let last = lines.len() - 2;
    lines.swap(1, last);
    let (status, why) = import(&target, "air", lines.join("\n") + "\n").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(why.contains("entities come first"), "{why}");

    // Left as it was found: the tenant did not exist, and does not.
    let (status, _) = call(&target, "GET", "/q/tenants/air/digest", "", None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_export_refuses_what_it_does_not_carry_and_names_it() {
    let st = state().await;
    seeded(&st, "subscribed").await;
    let sub = json!({"id": "urn:ngsi-ld:Subscription:1", "type": "Subscription", "entities": [{"type": "Room"}],
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/notify"}}});
    assert_eq!(
        ngsi(&st, "POST", "/ngsi-ld/v1/subscriptions", sub, "subscribed").await,
        StatusCode::CREATED
    );
    let (status, body) = call(&st, "GET", "/q/tenants/subscribed/export", "", None, false).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(String::from_utf8_lossy(&body).contains("1 subscriptions"));

    for path in ["/q/tenants/nobody/export", "/q/tenants/nobody/digest"] {
        assert_eq!(
            call(&st, "GET", path, "", None, false).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        call(&st, "GET", "/q/tenants/Bad%20Name/digest", "", None, false)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

/// The move between backends: a memory broker's tenant imported into a
/// Postgres one answers the same digest and the same history.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live PostGIS in ANTARES_TEST_DATABASE_URL: cargo test -p antares-api --features postgres --test it tenant_move:: -- --ignored"]
async fn a_memory_tenant_moves_into_postgres_with_the_same_digest() {
    let url = std::env::var("ANTARES_TEST_DATABASE_URL")
        .expect("ANTARES_TEST_DATABASE_URL must point at a live PostGIS");
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let store =
        antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(pool));
    let mut target =
        AppState::with_store("antares-pg".into(), std::sync::Arc::new(store), "postgres");
    antares_api::wire(&mut target).await;
    let source = state().await;
    let tenant = format!("move{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    seeded(&source, &tenant).await;

    let before = read(&source, &format!("/q/tenants/{tenant}/digest"), None).await;
    let (status, why) = import(&target, &tenant, export(&source, &tenant).await).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{why}");
    let after = read(&target, &format!("/q/tenants/{tenant}/digest"), None).await;
    assert_eq!(after, before);
    let path = "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Room:1";
    assert_eq!(
        read(&target, path, Some(&tenant)).await,
        read(&source, path, Some(&tenant)).await
    );
    // and back out of Postgres: the export of the copy is the export of the source
    assert_eq!(
        export(&target, &tenant).await,
        export(&source, &tenant).await
    );
}

/// T-4016: a target holding anything, a Subscription or a hosted @context
/// as much as an Entity, is refused before a line is read, and what it holds
/// stays; a failed import removes only what it wrote.
#[tokio::test]
async fn an_import_never_touches_what_the_target_already_holds() {
    let (source, target) = (state().await, state().await);
    seeded(&source, "air").await;
    let stream = export(&source, "air").await;

    let sub = json!({"id": "urn:ngsi-ld:Subscription:kept", "type": "Subscription", "entities": [{"type": "Room"}],
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/notify"}}});
    assert_eq!(
        ngsi(&target, "POST", "/ngsi-ld/v1/subscriptions", sub, "air").await,
        StatusCode::CREATED
    );
    let ctx = json!({"@context": {"Room": "https://example.org/Room"}});
    assert_eq!(
        ngsi(&target, "POST", "/ngsi-ld/v1/jsonldContexts", ctx, "air").await,
        StatusCode::CREATED
    );
    let hosted = |st: AppState| async move {
        read(&st, "/ngsi-ld/v1/jsonldContexts?kind=Hosted", Some("air"))
            .await
            .as_array()
            .map_or(0, Vec::len)
    };

    let (status, why) = import(&target, "air", stream.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{why}");
    assert!(
        why.contains("1 subscriptions") && why.contains("1 @contexts"),
        "{why}"
    );
    // a cut stream into the same tenant: refused the same way, nothing purged
    let cut = stream.lines().take(3).collect::<Vec<_>>().join("\n") + "\n";
    let (status, _) = import(&target, "air", cut).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (s, _) = call(
        &target,
        "GET",
        "/ngsi-ld/v1/subscriptions/urn:ngsi-ld:Subscription:kept",
        "",
        Some("air"),
        false,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "the subscription is still there");
    assert_eq!(hosted(target.clone()).await, 1, "and so is the @context");
    let digest = read(&target, "/q/tenants/air/digest", None).await;
    assert_eq!(digest["entities"], 0, "{digest}");
}

#[tokio::test]
async fn a_known_empty_tenant_digests_to_zero() {
    let st = state().await;
    let sub = json!({"id": "urn:ngsi-ld:Subscription:1", "type": "Subscription", "entities": [{"type": "Room"}],
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/notify"}}});
    assert_eq!(
        ngsi(&st, "POST", "/ngsi-ld/v1/subscriptions", sub, "quiet").await,
        StatusCode::CREATED
    );
    assert_eq!(
        read(&st, "/q/tenants/quiet/digest", None).await,
        json!({"tenant": "quiet", "entities": 0, "attrInstances": 0, "sha256": "0".repeat(64)})
    );
}

/// A line that arrives one byte a chunk is read once (T-4016): the import
/// of an export split into one-byte chunks equals the import of it whole.
#[tokio::test]
async fn an_export_in_one_byte_chunks_imports_like_one_piece() {
    let (source, target) = (state().await, state().await);
    seeded(&source, "air").await;
    let stream = export(&source, "air").await;
    let chunks: Vec<Result<axum::body::Bytes, std::io::Error>> = stream
        .bytes()
        .map(|b| Ok(axum::body::Bytes::copy_from_slice(&[b])))
        .collect();
    let res = antares_api::router(target.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/q/tenants/air/import")
                .header("Content-Length", stream.len())
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        read(&target, "/q/tenants/air/digest", None).await,
        read(&source, "/q/tenants/air/digest", None).await
    );
}

/// F-R2ANT2-009: an Entity exported without history (retained away at the
/// source) arrives without history, so both sides digest alike.
#[tokio::test]
async fn an_entity_without_history_imports_without_history() {
    let (source, target) = (state().await, state().await);
    seeded(&source, "bare").await;
    let tenant = antares_model::TenantId::new("bare").expect("tenant");
    assert!(source
        .temporal
        .delete(&tenant, "urn:ngsi-ld:Room:2")
        .await
        .expect("delete history"));
    let before = read(&source, "/q/tenants/bare/digest", None).await;

    let (status, why) = import(&target, "bare", export(&source, "bare").await).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{why}");
    assert_eq!(read(&target, "/q/tenants/bare/digest", None).await, before);
}

/// F-R2ANT2-002: with ANTARES_TEMPORAL naming a second backend the history
/// is not in the store the export pages, so it refuses rather than drop it.
#[tokio::test]
async fn an_export_refuses_history_held_by_a_second_backend() {
    let mut st = AppState::new("antares-test".into());
    st.temporal = AppState::new("second".into()).temporal.clone();
    st.temporal_name = Some("second".into());
    antares_api::wire(&mut st).await;
    seeded(&st, "split").await;
    let (status, body) = call(&st, "GET", "/q/tenants/split/export", "", None, false).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(String::from_utf8_lossy(&body).contains("ANTARES_TEMPORAL=second"));
}
