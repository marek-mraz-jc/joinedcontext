#![forbid(unsafe_code)]
//! The registry's answers when its store fails or the Organization's storage quota is used up:
//! a store error is a 500 `UNKNOWN` naming no store detail, never an absent image; a quota is a
//! 403 `DENIED` the client can act on. Plus the variants the journeys in `oci.rs` do not take
//! (a declared manifest media type, a token for a subject without a login, a second upload of one
//! generic file).

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use shako_api::ShakoState;
use shako_core::{
    Action, InstanceId, InstanceRecord, LocalAuthorizer, OrgId, OutboundGuard, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier, sha256_hex,
};
use shako_registry::oci_v2_routes;
use std::sync::Arc;
use tower::ServiceExt;

#[path = "../../api/tests/support/fault_store.rs"]
mod fault_store;
use fault_store::FaultStore;

const PUSHER: &str = "pusher-token-0123456789";
const NAMELESS: &str = "nameless-token-01234567";

fn org() -> OrgId {
    OrgId::from_u128(10)
}

struct Registry {
    app: Router,
    state: ShakoState,
    store: Arc<FaultStore>,
}

fn registry(tmp: &tempfile::TempDir) -> Registry {
    let store = Arc::new(FaultStore::default());
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        tmp.path().to_path_buf(),
        store.clone(),
        Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org(),
            principal: Principal::User("pusher".into()),
            actions: vec![Action::Publish, Action::Read],
            project_id: None,
            repo: None,
        }])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(
                PUSHER,
                Subject::authenticated("pusher", "pusher", vec![org()]),
            ),
            // A token whose subject carries an Organization but no login.
            StaticTokenBinding::new(NAMELESS, Subject::authenticated("", "", vec![org()])),
        ]),
        OutboundGuard::default(),
    );
    let app = oci_v2_routes().with_state(state.clone());
    Registry { app, state, store }
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Answer {
    fn code(&self) -> &str {
        self.body["errors"][0]["code"].as_str().unwrap_or_default()
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    extra: Option<(&str, &str)>,
    body: Vec<u8>,
) -> Answer {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some((name, value)) = extra {
        req = req.header(name, value);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    Answer {
        status: parts.status,
        headers: parts.headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn manifest(config: &str) -> Vec<u8> {
    json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": config, "digest": digest(b"cfg"), "size": 3},
        "layers": [],
    })
    .to_string()
    .into_bytes()
}

async fn push_blob(app: &Router, image: &str, bytes: &[u8]) -> Answer {
    let uri = format!("/v2/city/{image}/blobs/uploads/?digest={}", digest(bytes));
    send(app, Method::POST, &uri, PUSHER, None, bytes.to_vec()).await
}

async fn push_manifest(app: &Router, tag: &str, bytes: &[u8]) -> Answer {
    let uri = format!("/v2/city/app/manifests/{tag}");
    send(app, Method::PUT, &uri, PUSHER, None, bytes.to_vec()).await
}

fn assert_store_error(a: &Answer, what: &str) {
    assert_eq!(
        a.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{what}: {:?}",
        a.body
    );
    assert_eq!(a.code(), "UNKNOWN", "{what}");
    assert_eq!(a.body["errors"][0]["message"], "storage error", "{what}");
    assert!(
        !a.body.to_string().contains("refused by the test store"),
        "{what}: the store's own words stay in the log"
    );
}

/// Sets the Organization's storage limit to `bytes`.
async fn limit_storage(state: &ShakoState, bytes: u64) {
    let mut instance = InstanceRecord::default_for(InstanceId::from_u128(1), org(), "localhost");
    instance.max_storage_bytes = bytes;
    state
        .store()
        .upsert_instance(instance)
        .await
        .expect("instance");
}

#[tokio::test]
async fn a_blob_or_manifest_past_the_storage_quota_is_denied_with_the_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    limit_storage(&r.state, 4).await;

    let blob = push_blob(&r.app, "app", b"more than four bytes").await;
    assert_eq!(blob.status, StatusCode::FORBIDDEN, "{:?}", blob.body);
    assert_eq!(blob.code(), "DENIED");
    assert_eq!(
        blob.body["errors"][0]["message"],
        "the organization's storage quota is used up"
    );

    let man = push_manifest(&r.app, "v1", &manifest("application/x")).await;
    assert_eq!(man.status, StatusCode::FORBIDDEN, "{:?}", man.body);
    assert_eq!(man.code(), "DENIED");
    // Nothing was published: the tag is unknown.
    let read = send(
        &r.app,
        Method::GET,
        "/v2/city/app/manifests/v1",
        PUSHER,
        None,
        vec![],
    )
    .await;
    assert_eq!(read.status, StatusCode::NOT_FOUND);
    assert_eq!(read.code(), "MANIFEST_UNKNOWN");
}

#[tokio::test]
async fn a_manifest_whose_signature_does_not_fit_the_quota_is_stored_unsigned() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    let bytes = manifest("application/vnd.oci.image.config.v1+json");
    // Room for the manifest, none for the signature payload stored after it.
    limit_storage(&r.state, bytes.len() as u64).await;

    let man = push_manifest(&r.app, "v1", &bytes).await;
    assert_eq!(man.status, StatusCode::CREATED, "{:?}", man.body);
    assert_eq!(
        man.headers["docker-content-digest"],
        digest(&bytes).as_str()
    );
    let version = r
        .state
        .store()
        .get_package_version(
            InstanceId::from_u128(1),
            org(),
            "city",
            "container",
            "app",
            "v1",
        )
        .await
        .expect("read")
        .expect("the version");
    assert!(!version.signed, "no signature was stored");
    let sig_tag = format!("sha256-{}.sig", sha256_hex(&bytes));
    let sig = send(
        &r.app,
        Method::GET,
        &format!("/v2/city/app/manifests/{sig_tag}"),
        PUSHER,
        None,
        vec![],
    )
    .await;
    assert_eq!(sig.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_declared_content_type_is_the_manifest_media_type() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    let bytes = manifest("application/x");
    let declared = "application/vnd.docker.distribution.manifest.v2+json";
    let man = send(
        &r.app,
        Method::PUT,
        "/v2/city/app/manifests/v1",
        PUSHER,
        Some(("content-type", declared)),
        bytes,
    )
    .await;
    assert_eq!(man.status, StatusCode::CREATED, "{:?}", man.body);
    let version = r
        .state
        .store()
        .get_package_version(
            InstanceId::from_u128(1),
            org(),
            "city",
            "container",
            "app",
            "v1",
        )
        .await
        .expect("read")
        .expect("the version");
    assert_eq!(version.media_type, declared);
    assert!(version.signed);
}

#[tokio::test]
async fn every_store_failure_of_a_push_is_a_storage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    // A blob pushed whole, under a store that cannot read the Organization's quota.
    r.store.fail("get_instance");
    assert_store_error(&push_blob(&r.app, "app", b"layer").await, "blob");
    assert_store_error(
        &push_manifest(&r.app, "v0", &manifest("application/x")).await,
        "manifest blob",
    );
    r.store.heal();

    // Each later write of a manifest push; the tag differs so no earlier attempt answers.
    for (n, call) in [
        "upsert_package",
        "upsert_package_version",
        "append_audit",
        "append_outbox",
    ]
    .into_iter()
    .enumerate()
    {
        r.store.fail(call);
        let a = push_manifest(&r.app, &format!("v{}", n + 1), &manifest(call)).await;
        r.store.heal();
        assert_store_error(&a, call);
    }

    // A cross-repository mount whose new blob row cannot be written.
    let pushed = push_blob(&r.app, "base", b"shared layer").await;
    assert_eq!(pushed.status, StatusCode::CREATED, "{:?}", pushed.body);
    r.store.fail("upsert_blob");
    let uri = format!(
        "/v2/city/app/blobs/uploads/?mount={}&from=base",
        digest(b"shared layer")
    );
    let mount = send(&r.app, Method::POST, &uri, PUSHER, None, vec![]).await;
    r.store.heal();
    assert_store_error(&mount, "mount");
}

#[tokio::test]
async fn every_store_failure_of_a_pull_is_a_storage_error_not_an_absent_image() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    let bytes = manifest("application/x");
    let man = push_manifest(&r.app, "v1", &bytes).await;
    assert_eq!(man.status, StatusCode::CREATED, "{:?}", man.body);
    let referrers = format!("/v2/city/app/referrers/{}", digest(&bytes));
    for (call, uri) in [
        ("get_package_version", "/v2/city/app/manifests/v1"),
        ("get_blob", "/v2/city/app/manifests/v1"),
        ("list_package_tags", "/v2/city/app/tags/list"),
        ("list_referrers", referrers.as_str()),
    ] {
        r.store.fail(call);
        let a = send(&r.app, Method::GET, uri, PUSHER, None, vec![]).await;
        r.store.heal();
        assert_store_error(&a, call);
    }
    // Healed, the same reads answer.
    let read = send(
        &r.app,
        Method::GET,
        "/v2/city/app/manifests/v1",
        PUSHER,
        None,
        vec![],
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);
}

#[tokio::test]
async fn a_tag_whose_manifest_blob_is_gone_is_an_unknown_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    let man = push_manifest(&r.app, "v1", &manifest("application/x")).await;
    assert_eq!(man.status, StatusCode::CREATED, "{:?}", man.body);
    let store = r.state.store();
    let mut ghost = store
        .get_package_version(
            InstanceId::from_u128(1),
            org(),
            "city",
            "container",
            "app",
            "v1",
        )
        .await
        .expect("read")
        .expect("the version");
    // A tag row naming a digest no blob row holds (a blob swept after its tag was written).
    ghost.id = uuid::Uuid::now_v7();
    ghost.version = "ghost".into();
    ghost.digest = digest(b"no such manifest");
    store.upsert_package_version(ghost).await.expect("tag row");
    for method in [Method::GET, Method::HEAD] {
        let a = send(
            &r.app,
            method.clone(),
            "/v2/city/app/manifests/ghost",
            PUSHER,
            None,
            vec![],
        )
        .await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{method}");
        if method == Method::GET {
            assert_eq!(a.code(), "MANIFEST_UNKNOWN");
        }
    }
}

#[tokio::test]
async fn a_token_for_a_subject_without_a_login_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    let a = send(
        &r.app,
        Method::GET,
        "/v2/token?service=shako",
        NAMELESS,
        None,
        vec![],
    )
    .await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{:?}", a.body);
    assert!(a.body.get("token").is_none(), "{:?}", a.body);
    assert_eq!(a.headers[header::WWW_AUTHENTICATE], "Basic realm=\"shako\"");
}

#[tokio::test]
async fn a_generic_file_uploaded_twice_is_a_conflict_and_keeps_the_first() {
    let tmp = tempfile::tempdir().unwrap();
    let r = registry(&tmp);
    let uri = "/api/packages/city/generic/tools/1.0.0/tool.bin";
    let first = send(&r.app, Method::PUT, uri, PUSHER, None, b"first".to_vec()).await;
    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    let second = send(&r.app, Method::PUT, uri, PUSHER, None, b"second".to_vec()).await;
    assert_eq!(second.status, StatusCode::CONFLICT, "{:?}", second.body);
    assert_eq!(second.body["message"], "package file already exists");
    let read = r
        .app
        .clone()
        .oneshot(
            Request::get(uri)
                .header(header::AUTHORIZATION, format!("Bearer {PUSHER}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let body = read.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"first");
}
