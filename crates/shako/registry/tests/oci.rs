#![forbid(unsafe_code)]
//! The OCI registry and generic packages as a client meets them: ping and token, blob uploads
//! (monolithic, chunked, mounted), manifests, tags with paging, referrers, and every refusal
//! (401 with the token challenge, 403, 404 for what the caller may not see, 400, 413).

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use shako_api::ShakoState;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, Principal, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier, sha256_hex,
};
use shako_registry::{oci_v2_routes, stripped_v2_registry_routes};
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const PUSHER: &str = "pusher-token-0123456789";
const PULLER: &str = "puller-token-0123456789";
const STRANGER: &str = "stranger-token-012345678";

fn state(tmp: &tempfile::TempDir) -> ShakoState {
    let org = OrgId::from_u128(10);
    let bind = |user: &str, actions: Vec<Action>| RoleBinding {
        org_id: org,
        principal: Principal::User(user.into()),
        actions,
        project_id: None,
        repo: None,
    };
    let subject = |user: &str, orgs: Vec<OrgId>| Subject::authenticated(user, user, orgs);
    ShakoState::new(
        InstanceId::from_u128(1),
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        Arc::new(LocalAuthorizer::new(vec![
            bind("pusher", vec![Action::Publish, Action::Read]),
            bind("puller", vec![Action::Read]),
        ])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(PUSHER, subject("pusher", vec![org])),
            StaticTokenBinding::new(PULLER, subject("puller", vec![org])),
            StaticTokenBinding::new(STRANGER, subject("stranger", Vec::new())),
        ]),
        OutboundGuard::default(),
    )
}

fn app(tmp: &tempfile::TempDir) -> Router {
    oci_v2_routes().with_state(state(tmp))
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn code(&self) -> String {
        self.json()["errors"][0]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default()
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    Answer {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

async fn push_blob(app: &Router, image: &str, bytes: &[u8]) -> String {
    let d = digest(bytes);
    let uri = format!("/v2/city/{image}/blobs/uploads/?digest={d}");
    let a = send(app, Method::POST, &uri, Some(PUSHER), bytes.to_vec()).await;
    assert_eq!(
        a.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&a.body)
    );
    assert_eq!(a.header("location"), format!("/v2/city/{image}/blobs/{d}"));
    d
}

fn manifest(config: &str, subject: Option<&str>) -> Vec<u8> {
    let mut m = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config, "size": 2},
        "layers": []
    });
    if let Some(s) = subject {
        m["subject"] = json!({"digest": s});
        m["artifactType"] = json!("application/example.sbom");
    }
    serde_json::to_vec(&m).unwrap()
}

#[tokio::test]
async fn the_ping_answers_a_caller_and_challenges_anyone_else() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp).layer(Extension(Mount {
        scheme: "https".into(),
        host: "forge.example".into(),
        ..Mount::default()
    }));
    for uri in ["/v2", "/v2/"] {
        let ok = send(&app, Method::GET, uri, Some(PULLER), Vec::new()).await;
        assert_eq!(ok.status, StatusCode::OK);
        assert_eq!(ok.header("docker-distribution-api-version"), "registry/2.0");
        let no = send(&app, Method::GET, uri, None, Vec::new()).await;
        assert_eq!(no.status, StatusCode::UNAUTHORIZED);
        assert_eq!(no.code(), "UNAUTHORIZED");
        assert_eq!(
            no.header("www-authenticate"),
            "Bearer realm=\"https://forge.example/v2/token\",service=\"container_registry\""
        );
    }
    // Every other 401 carries the same challenge, so a client can refresh its token mid-push.
    let tags = send(
        &app,
        Method::GET,
        "/v2/city/app/tags/list",
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(tags.status, StatusCode::UNAUTHORIZED);
    assert!(
        tags.header("www-authenticate")
            .contains("forge.example/v2/token")
    );
    // Without a mount the challenge has no realm to name.
    let bare = oci_v2_routes().with_state(state(&tmp));
    let tags = send(
        &bare,
        Method::GET,
        "/v2/city/app/tags/list",
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(tags.header("www-authenticate"), "");
}

#[tokio::test]
async fn a_token_carries_only_the_scopes_the_caller_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let token = |q: &str, t: Option<&'static str>| {
        let (app, uri) = (app.clone(), format!("/v2/token{q}"));
        async move { send(&app, Method::GET, &uri, t, Vec::new()).await }
    };
    let pushed = token(
        "?service=container_registry&scope=repository:city/app:pull,push,delete&scope=registry:catalog:*&scope=repository:bad&scope=repository:noslash:pull",
        Some(PUSHER),
    )
    .await;
    assert_eq!(pushed.status, StatusCode::OK);
    let jwt = pushed.json()["token"].as_str().unwrap().to_owned();
    assert_eq!(pushed.json()["access_token"], pushed.json()["token"]);
    assert_eq!(pushed.json()["expires_in"], 900);
    // The minted token opens the registry for the scope it carries.
    let ping = send(&app, Method::GET, "/v2/", Some(&jwt), Vec::new()).await;
    assert_eq!(ping.status, StatusCode::OK);
    let pulled = token(
        "?scope=repository%3Acity%2Fapp%3Apull+repository:city/b:pull",
        Some(PULLER),
    )
    .await;
    assert_eq!(pulled.status, StatusCode::OK);
    let only_push = token("?scope=repository:city/app:push", Some(PULLER)).await;
    assert_eq!(
        (only_push.status, only_push.code().as_str()),
        (StatusCode::FORBIDDEN, "DENIED")
    );
    let no_scope = token("", Some(PULLER)).await;
    assert_eq!(
        no_scope.status,
        StatusCode::OK,
        "a login without scope gets a token"
    );
    let homeless = token("", Some(STRANGER)).await;
    assert_eq!(homeless.status, StatusCode::FORBIDDEN);
    for (q, t, status, msg) in [
        (
            "?scope=x&color=red",
            Some(PULLER),
            StatusCode::BAD_REQUEST,
            "unknown token request parameter 'color'",
        ),
        (
            "?scope=%FF",
            Some(PULLER),
            StatusCode::BAD_REQUEST,
            "token request parameters must be UTF-8",
        ),
        ("", None, StatusCode::UNAUTHORIZED, "missing credentials"),
        (
            "",
            Some("not-a-token"),
            StatusCode::UNAUTHORIZED,
            "invalid token",
        ),
    ] {
        let a = token(q, t).await;
        assert_eq!(a.status, status, "{q}");
        assert_eq!(a.json()["errors"][0]["message"], msg, "{q}");
    }
    let mut req = Request::builder().uri("/v2/token");
    req = req.header(header::AUTHORIZATION, "Digest abc");
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "an unknown scheme is no credential"
    );
    let post = send(&app, Method::POST, "/v2/token", None, Vec::new()).await;
    assert_eq!(
        post.status,
        StatusCode::NOT_FOUND,
        "containerd falls back to GET"
    );
}

#[tokio::test]
async fn a_blob_is_pushed_whole_and_read_back_by_head_and_get() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let d = push_blob(&app, "app", b"layer-bytes").await;
    let uri = format!("/v2/city/app/blobs/{d}");
    let head = send(&app, Method::HEAD, &uri, Some(PULLER), Vec::new()).await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.header("docker-content-digest"), d);
    assert_eq!(head.header("content-length"), "11");
    let got = send(&app, Method::GET, &uri, Some(PULLER), Vec::new()).await;
    assert_eq!(
        (got.status, got.body.as_slice()),
        (StatusCode::OK, &b"layer-bytes"[..])
    );
    // An upper-case digest names the same blob.
    let upper = format!("/v2/city/app/blobs/sha256:{}", d[7..].to_ascii_uppercase());
    assert_eq!(
        send(&app, Method::GET, &upper, Some(PULLER), Vec::new())
            .await
            .status,
        StatusCode::OK
    );
    for (uri, token, status, code) in [
        (uri.clone(), None, StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
        (
            uri.clone(),
            Some(STRANGER),
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        ),
        (
            format!("/v2/BAD%20OWNER/app/blobs/{d}"),
            Some(PULLER),
            StatusCode::NOT_FOUND,
            "NAME_UNKNOWN",
        ),
        (
            "/v2/city/app/blobs/md5:abc".into(),
            Some(PULLER),
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
        ),
        (
            "/v2/city/app/blobs/sha256:zz".into(),
            Some(PULLER),
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
        ),
        (
            format!("/v2/city/other/blobs/{d}"),
            Some(PULLER),
            StatusCode::NOT_FOUND,
            "BLOB_UNKNOWN",
        ),
    ] {
        let a = send(&app, Method::GET, &uri, token, Vec::new()).await;
        assert_eq!((a.status, a.code()), (status, code.to_owned()), "{uri}");
    }
    // The blob row without its file reads as unknown, never as an empty blob.
    std::fs::remove_file(shako_api::service::blob_disk_path(&state(&tmp), "city", &d)).unwrap();
    let gone = send(&app, Method::GET, &uri, Some(PULLER), Vec::new()).await;
    assert_eq!(
        (gone.status, gone.code().as_str()),
        (StatusCode::NOT_FOUND, "BLOB_UNKNOWN")
    );
}

#[tokio::test]
async fn a_monolithic_upload_is_refused_when_its_bytes_or_name_do_not_hold() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let wrong = digest(b"other");
    for (uri, token, status, code) in [
        (
            format!("/v2/city/app/blobs/uploads/?digest={wrong}"),
            Some(PUSHER),
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
        ),
        (
            "/v2/city/app/blobs/uploads/?digest=sha1:abc".into(),
            Some(PUSHER),
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
        ),
        (
            format!("/v2/city/app/blobs/uploads/?digest={wrong}"),
            Some(PULLER),
            StatusCode::NOT_FOUND,
            "NAME_UNKNOWN",
        ),
        (
            format!("/v2/city/app/blobs/uploads/?digest={wrong}"),
            None,
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        ),
    ] {
        let a = send(&app, Method::POST, &uri, token, b"bytes".to_vec()).await;
        assert_eq!((a.status, a.code()), (status, code.to_owned()), "{uri}");
    }
    for uri in [
        "/v2/bad%20owner/app/blobs/uploads",
        "/v2/city/..%2Fx/blobs/uploads",
    ] {
        let a = send(&app, Method::POST, uri, Some(PUSHER), Vec::new()).await;
        assert!(a.status.is_client_error(), "{uri}: {}", a.status);
        assert_ne!(a.status, StatusCode::ACCEPTED, "{uri}");
    }
    let big = vec![0u8; shako_registry::MAX_PACKAGE_BLOB_BYTES + 1];
    let a = send(
        &app,
        Method::POST,
        &format!("/v2/city/app/blobs/uploads/?digest={wrong}"),
        Some(PUSHER),
        big,
    )
    .await;
    assert_eq!(
        (a.status, a.code().as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "SIZE_INVALID")
    );
}

#[tokio::test]
async fn a_chunked_upload_collects_its_chunks_and_checks_the_final_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let start = send(
        &app,
        Method::POST,
        "/v2/city/app/blobs/uploads",
        Some(PUSHER),
        b"ab".to_vec(),
    )
    .await;
    assert_eq!(start.status, StatusCode::ACCEPTED);
    assert_eq!(start.header("range"), "0-1");
    let loc = start.header("location");
    assert_eq!(
        loc,
        format!(
            "/v2/city/app/blobs/uploads/{}",
            start.header("docker-upload-uuid")
        )
    );
    let patched = send(&app, Method::PATCH, &loc, Some(PUSHER), b"cd".to_vec()).await;
    assert_eq!(
        (patched.status, patched.header("range").as_str()),
        (StatusCode::ACCEPTED, "0-3")
    );
    // A wrong address never touches the upload.
    let elsewhere = loc.replace("/app/", "/other/");
    let a = send(&app, Method::PATCH, &elsewhere, Some(PUSHER), b"x".to_vec()).await;
    assert_eq!(
        (a.status, a.code().as_str()),
        (StatusCode::NOT_FOUND, "BLOB_UPLOAD_UNKNOWN")
    );
    let a = send(
        &app,
        Method::PUT,
        &format!("{elsewhere}?digest={}", digest(b"abcde")),
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(
        (a.status, a.code().as_str()),
        (StatusCode::NOT_FOUND, "BLOB_UPLOAD_UNKNOWN")
    );
    for (uri, status, code) in [
        (loc.clone(), StatusCode::BAD_REQUEST, "DIGEST_INVALID"),
        (
            format!("{loc}?digest=nope"),
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
        ),
    ] {
        let a = send(&app, Method::PUT, &uri, Some(PUSHER), Vec::new()).await;
        assert_eq!((a.status, a.code()), (status, code.to_owned()), "{uri}");
    }
    let done = send(
        &app,
        Method::PUT,
        &format!("{loc}?digest={}", digest(b"abcde")),
        Some(PUSHER),
        b"e".to_vec(),
    )
    .await;
    assert_eq!(done.status, StatusCode::CREATED);
    assert_eq!(done.header("docker-content-digest"), digest(b"abcde"));
    // The upload is gone once finished.
    let again = send(&app, Method::PATCH, &loc, Some(PUSHER), b"x".to_vec()).await;
    assert_eq!(again.code(), "BLOB_UPLOAD_UNKNOWN");
    // An empty start reports the range 0-0; a final digest that does not match is refused.
    let empty = send(
        &app,
        Method::POST,
        "/v2/city/app/blobs/uploads/",
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(empty.header("range"), "0-0");
    let bad = send(
        &app,
        Method::PUT,
        &format!("{}?digest={}", empty.header("location"), digest(b"zz")),
        Some(PUSHER),
        b"y".to_vec(),
    )
    .await;
    assert_eq!(
        (bad.status, bad.code().as_str()),
        (StatusCode::BAD_REQUEST, "DIGEST_INVALID")
    );
    for method in [Method::PATCH, Method::PUT] {
        let uri = format!("{loc}?digest={}", digest(b"x"));
        let a = send(&app, method.clone(), &uri, None, Vec::new()).await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{method}");
        let a = send(&app, method.clone(), &uri, Some(PULLER), Vec::new()).await;
        assert_eq!(
            (a.status, a.code().as_str()),
            (StatusCode::NOT_FOUND, "NAME_UNKNOWN"),
            "{method}"
        );
    }
}

#[tokio::test]
async fn a_chunk_past_the_package_limit_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let max = shako_registry::MAX_PACKAGE_BLOB_BYTES;
    let start = send(
        &app,
        Method::POST,
        "/v2/city/app/blobs/uploads",
        Some(PUSHER),
        vec![1; max],
    )
    .await;
    let loc = start.header("location");
    let a = send(&app, Method::PATCH, &loc, Some(PUSHER), vec![1]).await;
    assert_eq!(
        (a.status, a.json()["errors"][0]["message"].as_str()),
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("blob exceeds the package size limit")
        )
    );
    let a = send(&app, Method::PATCH, &loc, Some(PUSHER), vec![1; max + 1]).await;
    assert_eq!(a.json()["errors"][0]["message"], "chunk too large");
    let a = send(
        &app,
        Method::PUT,
        &format!("{loc}?digest={}", digest(b"x")),
        Some(PUSHER),
        vec![1; max + 1],
    )
    .await;
    assert_eq!(a.json()["errors"][0]["message"], "blob too large");
    let a = send(
        &app,
        Method::PUT,
        &format!("{loc}?digest={}", digest(b"x")),
        Some(PUSHER),
        vec![1],
    )
    .await;
    assert_eq!(
        a.json()["errors"][0]["message"],
        "blob exceeds the package size limit"
    );
}

#[tokio::test]
async fn a_blob_mounts_from_an_image_of_the_same_owner_only() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let d = push_blob(&app, "base", b"shared-layer").await;
    let mounted = send(
        &app,
        Method::POST,
        &format!("/v2/city/app/blobs/uploads/?mount={d}&from=city/base"),
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(mounted.status, StatusCode::CREATED);
    assert_eq!(
        mounted.header("location"),
        format!("/v2/city/app/blobs/{d}")
    );
    let read = send(
        &app,
        Method::HEAD,
        &format!("/v2/city/app/blobs/{d}"),
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);
    // `from` without an owner names an image of the same owner.
    let short = send(
        &app,
        Method::POST,
        &format!("/v2/city/app2/blobs/uploads/?mount={d}&from=base"),
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(short.status, StatusCode::CREATED);
    let cross = send(
        &app,
        Method::POST,
        &format!("/v2/city/app/blobs/uploads/?mount={d}&from=town/base"),
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(
        (cross.status, cross.code().as_str()),
        (StatusCode::FORBIDDEN, "DENIED")
    );
    let bad = send(
        &app,
        Method::POST,
        "/v2/city/app/blobs/uploads/?mount=x&from=city/base",
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(
        (bad.status, bad.code().as_str()),
        (StatusCode::BAD_REQUEST, "DIGEST_INVALID")
    );
    // A blob the source does not hold is uploaded as usual: the answer opens a chunked upload.
    let missing = digest(b"nowhere");
    let fallback = send(
        &app,
        Method::POST,
        &format!("/v2/city/app/blobs/uploads/?mount={missing}&from=city/base"),
        Some(PUSHER),
        Vec::new(),
    )
    .await;
    assert_eq!(fallback.status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn a_manifest_is_stored_signed_and_listed_by_tag_and_referrer() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let config = push_blob(&app, "app", b"{}").await;
    let body = manifest(&config, None);
    let md = digest(&body);
    let put = send(
        &app,
        Method::PUT,
        "/v2/city/app/manifests/v1",
        Some(PUSHER),
        body.clone(),
    )
    .await;
    assert_eq!(
        put.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&put.body)
    );
    assert_eq!(
        put.header("location"),
        format!("/v2/city/app/manifests/{md}")
    );
    for reference in ["v1", md.as_str()] {
        let uri = format!("/v2/city/app/manifests/{reference}");
        let got = send(&app, Method::GET, &uri, Some(PULLER), Vec::new()).await;
        assert_eq!(got.status, StatusCode::OK, "{reference}");
        assert_eq!(got.body, body);
        assert_eq!(
            got.header("content-type"),
            "application/vnd.oci.image.manifest.v1+json"
        );
        let head = send(&app, Method::HEAD, &uri, Some(PULLER), Vec::new()).await;
        assert_eq!(
            (head.status, head.header("docker-content-digest")),
            (StatusCode::OK, md.clone())
        );
        assert!(head.body.is_empty());
    }
    // The forge signs every pushed manifest: its signature is a referrer of the digest.
    let referrers = send(
        &app,
        Method::GET,
        &format!("/v2/city/app/referrers/{md}"),
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(referrers.status, StatusCode::OK);
    assert_eq!(
        referrers.header("content-type"),
        "application/vnd.oci.image.index.v1+json"
    );
    let index = referrers.json();
    assert_eq!(
        index["manifests"][0]["artifactType"],
        "application/vnd.dev.cosign.simplesigning.v1+json"
    );
    // A pushed artifact naming the manifest as subject is listed beside it.
    let sbom = manifest(&config, Some(&md));
    let sd = digest(&sbom);
    let put = send(
        &app,
        Method::PUT,
        &format!("/v2/city/app/manifests/{sd}"),
        Some(PUSHER),
        sbom,
    )
    .await;
    assert_eq!(put.status, StatusCode::CREATED);
    let index = send(
        &app,
        Method::GET,
        &format!("/v2/city/app/referrers/{md}"),
        Some(PULLER),
        Vec::new(),
    )
    .await
    .json();
    let types: Vec<&str> = index["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["artifactType"].as_str())
        .collect();
    assert!(types.contains(&"application/example.sbom"), "{index}");
    let none = send(
        &app,
        Method::GET,
        &format!("/v2/city/app/referrers/{}", digest(b"z")),
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(none.json()["manifests"], json!([]));
    for (uri, token, status, code) in [
        (
            "/v2/city/app/manifests/v9".to_string(),
            Some(PULLER),
            StatusCode::NOT_FOUND,
            "MANIFEST_UNKNOWN",
        ),
        (
            "/v2/city/app/manifests/v1".into(),
            None,
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        ),
        (
            "/v2/BAD%20OWNER/app/manifests/v1".into(),
            Some(PULLER),
            StatusCode::NOT_FOUND,
            "NAME_UNKNOWN",
        ),
        (
            format!("/v2/city/app/referrers/{md}"),
            None,
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        ),
        (
            format!("/v2/BAD%20OWNER/app/referrers/{md}"),
            Some(PULLER),
            StatusCode::NOT_FOUND,
            "NAME_UNKNOWN",
        ),
    ] {
        let a = send(&app, Method::GET, &uri, token, Vec::new()).await;
        assert_eq!((a.status, a.code()), (status, code.to_owned()), "{uri}");
    }
    // A manifest whose file no longer matches its digest is a storage error, never served.
    std::fs::write(
        shako_api::service::blob_disk_path(&state(&tmp), "city", &md),
        b"tampered",
    )
    .unwrap();
    let a = send(
        &app,
        Method::GET,
        "/v2/city/app/manifests/v1",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(
        (a.status, a.code().as_str()),
        (StatusCode::INTERNAL_SERVER_ERROR, "UNKNOWN")
    );
    let file = shako_api::service::blob_disk_path(&state(&tmp), "city", &md);
    std::fs::remove_file(&file).unwrap();
    let a = send(
        &app,
        Method::GET,
        "/v2/city/app/manifests/v1",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR);
    // A directory in its place opens and then fails to read: still a storage error.
    std::fs::create_dir(&file).unwrap();
    let a = send(
        &app,
        Method::GET,
        "/v2/city/app/manifests/v1",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(a.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn a_manifest_push_is_refused_when_it_is_no_json_or_names_another_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let body = manifest(&digest(b"{}"), None);
    let wrong = digest(b"other");
    for (uri, token, payload, status, code) in [
        (
            "/v2/city/app/manifests/v1".to_string(),
            Some(PUSHER),
            b"not json".to_vec(),
            StatusCode::BAD_REQUEST,
            "MANIFEST_INVALID",
        ),
        (
            format!("/v2/city/app/manifests/{wrong}"),
            Some(PUSHER),
            body.clone(),
            StatusCode::BAD_REQUEST,
            "DIGEST_INVALID",
        ),
        (
            "/v2/city/app/manifests/v1".into(),
            Some(PULLER),
            body.clone(),
            StatusCode::NOT_FOUND,
            "NAME_UNKNOWN",
        ),
        (
            "/v2/city/app/manifests/v1".into(),
            None,
            body.clone(),
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        ),
        (
            "/v2/city/app/manifests/v1".into(),
            Some(PUSHER),
            vec![b' '; shako_registry::MAX_OCI_MANIFEST_BYTES + 1],
            StatusCode::PAYLOAD_TOO_LARGE,
            "MANIFEST_INVALID",
        ),
    ] {
        let a = send(&app, Method::PUT, &uri, token, payload).await;
        assert_eq!((a.status, a.code()), (status, code.to_owned()), "{uri}");
    }
    for uri in [
        "/v2/bad%20owner/app/manifests/v1",
        "/v2/city/..%2Fx/manifests/v1",
        "/v2/city/app/manifests/..%2Fx",
    ] {
        let a = send(&app, Method::PUT, uri, Some(PUSHER), body.clone()).await;
        assert!(
            a.status.is_client_error() && a.status != StatusCode::CREATED,
            "{uri}: {}",
            a.status
        );
    }
    // A pushed signature tag is stored as given, unsigned by the forge.
    let sig = send(
        &app,
        Method::PUT,
        "/v2/city/app/manifests/sha256-abc.sig",
        Some(PUSHER),
        body,
    )
    .await;
    assert_eq!(sig.status, StatusCode::CREATED);
}

#[tokio::test]
async fn tags_are_listed_in_pages_with_a_link_to_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let config = push_blob(&app, "app", b"{}").await;
    for tag in ["a", "b", "c"] {
        let body = manifest(&config, None);
        let put = send(
            &app,
            Method::PUT,
            &format!("/v2/city/app/manifests/{tag}"),
            Some(PUSHER),
            body,
        )
        .await;
        assert_eq!(put.status, StatusCode::CREATED, "{tag}");
    }
    let all = send(
        &app,
        Method::GET,
        "/v2/city/app/tags/list",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(all.json()["name"], "city/app");
    let tags: Vec<String> = serde_json::from_value(all.json()["tags"].clone()).unwrap();
    assert!(
        tags.starts_with(&["a".to_string(), "b".into(), "c".into()]),
        "{tags:?}"
    );
    let page = send(
        &app,
        Method::GET,
        "/v2/city/app/tags/list?n=2",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(page.json()["tags"], json!(["a", "b"]));
    assert_eq!(
        page.header("link"),
        "</v2/city/app/tags/list?n=2&last=b>; rel=\"next\""
    );
    let rest = send(
        &app,
        Method::GET,
        "/v2/city/app/tags/list?n=1&last=b",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(rest.json()["tags"], json!(["c"]));
    let zero = send(
        &app,
        Method::GET,
        "/v2/city/app/tags/list?n=0",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(
        (zero.json()["tags"].clone(), zero.header("link")),
        (json!([]), String::new())
    );
    let last_page = send(
        &app,
        Method::GET,
        &format!("/v2/city/app/tags/list?n={}", tags.len()),
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(
        last_page.header("link"),
        "",
        "no next page past the last tag"
    );
    let hidden = send(
        &app,
        Method::GET,
        "/v2/BAD%20OWNER/app/tags/list",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(hidden.code(), "NAME_UNKNOWN");
}

#[tokio::test]
async fn the_stripped_router_serves_the_same_registry_without_the_v2_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let app = stripped_v2_registry_routes().with_state(state(&tmp));
    assert_eq!(
        send(&app, Method::GET, "/", Some(PULLER), Vec::new())
            .await
            .status,
        StatusCode::OK
    );
    let tags = send(
        &app,
        Method::GET,
        "/city/app/tags/list",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(tags.status, StatusCode::OK);
    let token = send(&app, Method::GET, "/token", Some(PULLER), Vec::new()).await;
    assert_eq!(token.status, StatusCode::OK);
}

#[tokio::test]
async fn a_generic_package_file_is_published_and_read_back() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp);
    let uri = "/api/packages/city/generic/tool/1.0/tool.bin";
    let put = send(&app, Method::PUT, uri, Some(PUSHER), b"binary".to_vec()).await;
    assert_eq!(
        put.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&put.body)
    );
    let got = send(&app, Method::GET, uri, Some(PULLER), Vec::new()).await;
    assert_eq!(
        (got.status, got.body.as_slice()),
        (StatusCode::OK, &b"binary"[..])
    );
    assert_eq!(got.header("content-type"), "application/octet-stream");
    for (method, uri, token, status) in [
        (Method::PUT, uri, None, StatusCode::UNAUTHORIZED),
        (Method::GET, uri, None, StatusCode::UNAUTHORIZED),
        (Method::PUT, uri, Some(PULLER), StatusCode::FORBIDDEN),
        (
            Method::PUT,
            "/api/packages/city/generic/tool/1.0/..",
            Some(PUSHER),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            Method::GET,
            "/api/packages/city/generic/tool/9.9/tool.bin",
            Some(PULLER),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let a = send(&app, method.clone(), uri, token, b"x".to_vec()).await;
        assert_eq!(a.status, status, "{method} {uri}");
    }
    let big = vec![0u8; shako_registry::MAX_PACKAGE_BLOB_BYTES + 1];
    // A body past the allowance is refused as the quota it would break.
    let a = send(
        &app,
        Method::PUT,
        "/api/packages/city/generic/tool/2.0/big.bin",
        Some(PUSHER),
        big,
    )
    .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let gone = send(
        &app,
        Method::GET,
        "/api/packages/city/generic/tool/2.0/big.bin",
        Some(PULLER),
        Vec::new(),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}
