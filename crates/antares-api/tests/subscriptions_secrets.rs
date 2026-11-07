// SPDX-License-Identifier: EUPL-1.2
//! A Subscription's `endpoint.receiverInfo` carries the credential its
//! notifications are sent with (6.3.8): no read answers it, and a client
//! writing back what it read keeps the stored credential (T-5565, CC-06).

use antares_api::AppState;
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:Subscription:t5565";
const TOKEN: &str = "Bearer the-webhook-token";

async fn send(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let bytes = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(axum::http::header::CONTENT_LENGTH, bytes.len().to_string());
    if !bytes.is_empty() {
        let ct = if method == "PATCH" {
            "application/merge-patch+json"
        } else {
            "application/json"
        };
        req = req.header(axum::http::header::CONTENT_TYPE, ct);
    }
    let res = antares_api::router(st.clone())
        .oneshot(req.body(Body::from(bytes)).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn subscribed() -> AppState {
    let st = AppState::new("antares-subscription-secrets".into());
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some(json!({
            "id": ID,
            "type": "Subscription",
            "entities": [{"type": "Building"}],
            "notification": {"endpoint": {
                "uri": "http://localhost:18099/notify",
                "receiverInfo": [
                    {"key": "Authorization", "value": TOKEN},
                    {"key": "X-Space", "value": "air-quality"}
                ]
            }}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    st
}

async fn stored_receiver_info(st: &AppState) -> Value {
    let doc = st
        .store
        .get(&TenantId::default(), Kind::Subscription, ID)
        .await
        .expect("store")
        .expect("the subscription is stored");
    doc["notification"]["endpoint"]["receiverInfo"].clone()
}

#[tokio::test]
async fn receiver_info_is_never_answered() {
    let st = subscribed().await;
    let masked = json!([
        {"key": "Authorization", "value": "***"},
        {"key": "X-Space", "value": "***"}
    ]);
    let (status, one) = send(&st, "GET", &format!("/ngsi-ld/v1/subscriptions/{ID}"), None).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["notification"]["endpoint"]["receiverInfo"], masked);
    let (status, all) = send(&st, "GET", "/ngsi-ld/v1/subscriptions", None).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all[0]["notification"]["endpoint"]["receiverInfo"], masked);
    for answer in [one, all] {
        assert!(
            !answer.to_string().contains("the-webhook-token"),
            "{answer}"
        );
    }
    assert_eq!(stored_receiver_info(&st).await[0]["value"], json!(TOKEN));
}

#[tokio::test]
async fn a_masked_value_written_back_keeps_the_stored_credential() {
    let st = subscribed().await;
    let (_, read) = send(&st, "GET", &format!("/ngsi-ld/v1/subscriptions/{ID}"), None).await;
    let mut notification = read["notification"].clone();
    // the client changes one header and writes the rest back as it read it
    notification["endpoint"]["receiverInfo"][1]["value"] = json!("parking");
    let (status, body) = send(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/subscriptions/{ID}"),
        Some(json!({ "notification": notification })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        stored_receiver_info(&st).await,
        json!([
            {"key": "Authorization", "value": TOKEN},
            {"key": "X-Space", "value": "parking"}
        ])
    );
}

/// T-5827: the stored credential stays with the stored uri. A masked value
/// written beside another uri is refused naming receiverInfo, never the value,
/// and nothing is written.
#[tokio::test]
async fn a_masked_value_does_not_follow_a_new_uri() {
    let st = subscribed().await;
    let (status, body) = send(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/subscriptions/{ID}"),
        Some(json!({"notification": {"endpoint": {
            "uri": "http://localhost:18098/elsewhere",
            "receiverInfo": [{"key": "Authorization", "value": "***"}]
        }}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("receiverInfo"), "{body}");
    assert!(!body.to_string().contains("the-webhook-token"), "{body}");
    let doc = st
        .store
        .get(&TenantId::default(), Kind::Subscription, ID)
        .await
        .expect("store")
        .expect("the subscription is stored");
    assert_eq!(
        doc["notification"]["endpoint"]["uri"],
        "http://localhost:18099/notify"
    );
    assert_eq!(stored_receiver_info(&st).await[0]["value"], json!(TOKEN));
}

/// 32 zero bytes in base64: a test key that is plainly not a secret.
const TEST_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// A receiver that answers 200 and hands every request's Authorization header
/// out on a channel.
async fn header_server() -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(move |headers: axum::http::HeaderMap| {
            let tx = tx.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let _ = tx.send(auth);
                StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://localhost:{port}/notify"), rx)
}

async fn send_as(
    st: &AppState,
    tenant: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let bytes = body.to_string();
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("NGSILD-Tenant", tenant)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// T-5838: with ANTARES_SECRET_KEY the stored row holds ciphertext, every
/// read through the store opens it, and the receiver still gets the
/// plaintext header. Skips loudly without ANTARES_TEST_DATABASE_URL.
#[tokio::test(flavor = "multi_thread")]
async fn receiver_info_is_stored_encrypted() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    antares_jsonld::allow_private_egress(true);
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let key = antares_sql::store::pg::seal::SecretKey::from_base64(TEST_KEY).expect("key");
    let store = antares_sql::store::any::AnyStore::Pg(
        antares_sql::store::any::PgBackend::new(pool.clone()).with_secret_key(key),
    );
    let mut st = AppState::with_store(
        "antares-sealed".into(),
        std::sync::Arc::new(store),
        "postgres",
    );
    antares_api::wire(&mut st).await;
    let tenant = format!("ts{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let (uri, mut received) = header_server().await;

    let (status, body) = send_as(
        &st,
        &tenant,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        json!({
            "id": ID,
            "type": "Subscription",
            "entities": [{"type": "Building"}],
            "notification": {"endpoint": {
                "uri": uri,
                "receiverInfo": [{"key": "Authorization", "value": TOKEN}]
            }}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let mut tx = pool.begin().await.expect("tx");
    antares_sql::sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(&tenant)
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    let row: String = antares_sql::sqlx::query_scalar(
        "SELECT subscription::text FROM subscriptions WHERE tenant_id = $1 AND id = $2",
    )
    .bind(&tenant)
    .bind(ID)
    .fetch_one(&mut *tx)
    .await
    .expect("the row");
    tx.commit().await.expect("commit");
    assert!(!row.contains("the-webhook-token"), "{row}");
    assert!(row.contains("enc:v1:"), "{row}");

    let tid = TenantId::new(&tenant).expect("tenant");
    let doc = st
        .store
        .get(&tid, Kind::Subscription, ID)
        .await
        .expect("store")
        .expect("stored");
    assert_eq!(
        doc["notification"]["endpoint"]["receiverInfo"][0]["value"],
        json!(TOKEN)
    );

    let (status, body) = send_as(
        &st,
        &tenant,
        "POST",
        "/ngsi-ld/v1/entities",
        json!({"id": "urn:ngsi-ld:Building:sealed", "type": "Building",
               "height": {"type": "Property", "value": 1}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let auth = tokio::time::timeout(std::time::Duration::from_secs(10), received.recv())
        .await
        .expect("a notification within 10 s")
        .expect("the receiver runs");
    assert_eq!(auth, TOKEN);
    // the database is shared: a broker started there without this key would
    // fail every read of the sealed row
    for table in ["entities", "subscriptions"] {
        antares_sql::sqlx::query(antares_sql::sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE tenant_id = $1"
        )))
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
    }
}
