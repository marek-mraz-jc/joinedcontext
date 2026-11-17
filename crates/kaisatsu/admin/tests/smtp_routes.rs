#![forbid(unsafe_code)]
//! The realm's SMTP server door (T-7152): who may read and set it, the password write-only, and
//! a test e-mail that never reaches a private address.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_admin::{AdminState, admin_router};
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sign_jwt};
use kaisatsu_oidc::{OidcState, OutboundGuard};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    app: axum::Router,
    store: Arc<MemoryStore>,
    kek: Kek,
}

impl Fixture {
    async fn new() -> Self {
        let kek = Kek::generate_for_tests();
        let store = Arc::new(MemoryStore::new(kek.clone()));
        store
            .upsert_realm(RealmConfig {
                id: "jc".into(),
                realm: "jc".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let oidc = OidcState {
            store: store.clone(),
            kek: kek.clone(),
            guard: OutboundGuard::default(),
            default_mount: Mount::default(),
            second_factor: Default::default(),
        };
        let app = admin_router(AdminState::new(oidc, Arc::new(LocalAuthorizer)));
        Self { app, store, kek }
    }

    async fn token(&self, sub: &str, extra: Value) -> String {
        let keys = self.store.list_signing_keys("jc").await.unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({
            "iss": "https://idm.localhost/realms/jc",
            "sub": sub,
            "azp": "portal-reconciler",
            "typ": "Bearer",
            "iat": now,
            "exp": now + 3600,
        });
        claims
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        sign_jwt(&keys[0], &self.kek, Some("JWT"), &claims).unwrap()
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(format!("/admin/realms/jc/smtp-server{path}"))
            .header("authorization", format!("Bearer {token}"));
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn settings(host: &str) -> Value {
    json!({
        "host": host, "port": 587, "tls": "starttls",
        "from": "noreply@example.org", "user": "mailer", "password": "smtp-s3cret"
    })
}

#[tokio::test]
async fn only_manage_smtp_or_a_realm_applier_reads_and_sets_it() {
    let f = Fixture::new().await;
    let reconciler = f
        .token(
            "svc-reconciler",
            json!({ "resource_access": { "realm-management": { "roles": ["manage-smtp"] } } }),
        )
        .await;
    let users_only = f
        .token("svc-users", json!({ "resource_access": { "realm-management": { "roles": ["manage-users", "query-groups"] } } }))
        .await;
    let org_admin = f.token("org-admin", json!({ "jc_org": ["bbsk"] })).await;

    assert_eq!(
        f.call(Method::GET, "", &reconciler, None).await.0,
        StatusCode::NOT_FOUND
    );
    for token in [&users_only, &org_admin] {
        assert_eq!(
            f.call(Method::GET, "", token, None).await.0,
            StatusCode::FORBIDDEN
        );
        let put = f
            .call(Method::PUT, "", token, Some(settings("smtp.example.org")))
            .await;
        assert_eq!(put.0, StatusCode::FORBIDDEN);
        let test = f
            .call(
                Method::POST,
                "/test",
                token,
                Some(json!({ "to": "ops@example.org" })),
            )
            .await;
        assert_eq!(test.0, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        f.call(Method::GET, "", "forged", None).await.0,
        StatusCode::UNAUTHORIZED
    );

    let (status, _) = f
        .call(
            Method::PUT,
            "",
            &reconciler,
            Some(settings("smtp.example.org")),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, read) = f.call(Method::GET, "", &reconciler, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        read,
        json!({ "host": "smtp.example.org", "port": 587, "tls": "starttls",
                "from": "noreply@example.org", "user": "mailer", "passwordSet": true })
    );
    // The password sits sealed in the store, and no admin event carries it.
    let stored = f
        .store
        .get_realm("jc")
        .await
        .unwrap()
        .unwrap()
        .smtp_server
        .unwrap();
    assert_eq!(stored.password, "smtp-s3cret");
    let events = format!("{:?}", f.store.list_admin_events("jc", None).await.unwrap());
    assert!(
        events.contains("REALM_SMTP") && !events.contains("smtp-s3cret"),
        "{events}"
    );

    // A save without a password keeps the stored one; a bad field is named.
    let mut again = settings("smtp2.example.org");
    again.as_object_mut().unwrap().remove("password");
    assert_eq!(
        f.call(Method::PUT, "", &reconciler, Some(again)).await.0,
        StatusCode::NO_CONTENT
    );
    let stored = f
        .store
        .get_realm("jc")
        .await
        .unwrap()
        .unwrap()
        .smtp_server
        .unwrap();
    assert_eq!(
        (stored.host.as_str(), stored.password.as_str()),
        ("smtp2.example.org", "smtp-s3cret")
    );
    let (status, problem) = f
        .call(Method::PUT, "", &reconciler, Some(settings("10.43.0.1")))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(problem.to_string().contains("host"), "{problem}");
}

#[tokio::test]
async fn a_test_e_mail_never_reaches_a_private_address() {
    let f = Fixture::new().await;
    let admin = f
        .token(
            "admin",
            json!({ "realm_access": { "roles": ["realm-admin"] } }),
        )
        .await;
    // Stored as a realm file may write it: a name that resolves to loopback.
    let mut realm = f.store.get_realm("jc").await.unwrap().unwrap();
    realm.smtp_server = Some(SmtpServerConfig {
        host: "localhost".into(),
        from: "noreply@example.org".into(),
        auth: "false".into(),
        ..SmtpServerConfig::default()
    });
    f.store.upsert_realm(realm).await.unwrap();
    let (status, result) = f
        .call(
            Method::POST,
            "/test",
            &admin,
            Some(json!({ "to": "ops@example.org" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["sent"], json!(false));
    assert!(
        result["reason"].as_str().unwrap().contains("private"),
        "{result}"
    );
    let (status, _) = f
        .call(
            Method::POST,
            "/test",
            &admin,
            Some(json!({ "to": "not an address" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
