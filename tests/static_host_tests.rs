//! The static apps host: what it serves, what it refuses, and the headers every app carries
//! (AP-12, AP-14, AP-17).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::apps::static_host::sri_sha384;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::reconciler::transitions::Transitions;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

const INDEX: &[u8] = b"<!doctype html><title>air quality</title>";
const BUNDLE_JS: &[u8] = b"console.log('air quality');";

/// One app directory with its bundle and the integrity manifest CI writes beside it.
fn app_root(case: &str, files: &[(&str, &[u8])]) -> tempdir::Dir {
    let dir = tempdir::Dir::new(case);
    let app = dir.path().join("air-quality");
    std::fs::create_dir_all(&app).expect("app dir");
    let mut digests = serde_json::Map::new();
    for (name, bytes) in files {
        if let Some(parent) = app.join(name).parent() {
            std::fs::create_dir_all(parent).expect("bundle folder");
        }
        std::fs::write(app.join(name), bytes).expect("bundle file");
        digests.insert((*name).into(), sri_sha384(bytes).into());
    }
    std::fs::write(
        app.join("integrity.json"),
        serde_json::to_vec(&digests).expect("manifest"),
    )
    .expect("write manifest");
    dir
}

/// The tiniest temp directory that cleans up after itself; the suite needs nothing more.
mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new(case: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "jc-apps-{case}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}

/// The same mirror with the build the lane wrote back (AP-13a).
fn mirror_with_build(spec: serde_json::Value, commit: &str) -> Arc<Mirror> {
    let mirror = mirror_with_app(spec);
    let mut envelope = mirror
        .get("ovzdusie", "App", "air-quality")
        .expect("the app");
    envelope.status = Some(joinedcontext_portal::resource::Status {
        phase: joinedcontext_portal::resource::Phase::Live,
        observed_revision: None,
        source_url: None,
        conditions: Vec::new(),
        build: Some(jc_core::Build {
            digest: format!("sha256:{}", "a1b2c3d4".repeat(8)),
            commit: commit.to_owned(),
            sdk_version: "0.4.1".to_owned(),
            built_at: chrono::Utc::now(),
            component: None,
        }),
        shard: None,
        domain_verification: None,
        jobs: Vec::new(),
        notifications: None,
    });
    mirror.upsert(envelope);
    mirror
}

fn mirror_with_app(spec: serde_json::Value) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "App".into(),
        metadata: ObjectMeta {
            name: "air-quality".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec,
        status: None,
    });
    mirror
}

fn app_spec(lifecycle: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "static",
        "source": { "path": "." },
        "build": { "node": "22" },
        "visibility": "public",
        "lifecycle": lifecycle,
        "dataNeeds": []
    })
}

async fn get_from(
    root: &std::path::Path,
    spec: serde_json::Value,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let config = Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        ..Config::for_tests()
    };
    let app = server::app(AppState::new(config, None).with_mirror(mirror_with_app(spec)));
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn a_published_app_serves_its_bundle_under_the_platform_host() {
    let dir = app_root("serves", &[("index.html", INDEX), ("bundle.js", BUNDLE_JS)]);

    let (status, headers, body) =
        get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX, "the app's own index, not the Portal SPA");
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));

    let (status, headers, body) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/bundle.js",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, BUNDLE_JS);
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .contains("javascript"));
}

#[tokio::test]
async fn every_app_response_carries_its_own_policy_and_not_the_portals() {
    let dir = app_root("csp", &[("index.html", INDEX)]);

    let (_, headers, _) = get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;

    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    // No apps origin of its own: the App shares the Portal's, so not even the Portal frames it.
    assert!(csp.ends_with("frame-ancestors 'none'"), "{csp}");
    assert!(csp.contains("connect-src 'self';"), "{csp}");
    assert!(
        !csp.contains("worker-src"),
        "the Portal's policy is not the app's: {csp}"
    );
    // SAMEORIGIN or DENY would refuse the Portal's frame (AP-122).
    assert!(headers.get(header::X_FRAME_OPTIONS).is_none());
    assert_eq!(headers["x-content-type-options"], "nosniff");
}

/// AP-142 (T-3327): a WebAssembly module is served as `application/wasm`, the policy lets the
/// browser compile it and nothing more, and a file whose name carries its hash is cached for good
/// while every other file is revalidated.
#[tokio::test]
async fn a_webassembly_module_is_served_as_wasm_and_a_hashed_file_is_kept() {
    let wasm: &[u8] = b"\0asm\x01\0\0\0";
    let dir = app_root(
        "wasm",
        &[
            ("index.html", INDEX),
            ("assets/compute_bg-Cx8f2kQ9.wasm", wasm),
            ("assets/index-B4tq81Zr.js", BUNDLE_JS),
            ("bundle.js", BUNDLE_JS),
        ],
    );
    let get = |uri: &'static str| get_from(dir.path(), app_spec("published"), uri);

    let (status, headers, body) = get("/apps/air-quality/assets/compute_bg-Cx8f2kQ9.wasm").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, wasm);
    assert_eq!(headers[header::CONTENT_TYPE], "application/wasm");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "private, max-age=31536000, immutable"
    );
    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(
        csp.contains("script-src 'self' 'wasm-unsafe-eval';"),
        "{csp}"
    );

    let (_, headers, _) = get("/apps/air-quality/assets/index-B4tq81Zr.js").await;
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "private, max-age=31536000, immutable"
    );
    // A name without a hash, and the index, can change under the same path on a republish.
    let (_, headers, _) = get("/apps/air-quality/bundle.js").await;
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    let (_, headers, _) = get("/apps/air-quality/").await;
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
}

#[tokio::test]
async fn an_embeddable_app_may_be_framed_by_the_named_origin() {
    let dir = app_root("embed", &[("index.html", INDEX)]);
    let mut spec = app_spec("published");
    spec["embeddable"] = serde_json::json!(true);
    spec["csp"] = serde_json::json!({ "frameAncestors": ["https://portal.example.sk"] });

    let (_, headers, _) = get_from(dir.path(), spec, "/apps/air-quality/").await;

    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(
        csp.ends_with("frame-ancestors https://portal.example.sk"),
        "{csp}"
    );
    assert!(headers.get(header::X_FRAME_OPTIONS).is_none());
}

#[tokio::test]
async fn an_unpublished_app_is_not_found_rather_than_forbidden() {
    // A draft, a preview and a retired app answer exactly what a name that never existed
    // answers, so the host discloses nothing about what is being worked on (AP-18).
    let dir = app_root("draft", &[("index.html", INDEX)]);

    for lifecycle in ["draft", "preview", "retired"] {
        let (status, _, _) = get_from(dir.path(), app_spec(lifecycle), "/apps/air-quality/").await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{lifecycle} must not be reachable"
        );
    }

    let (status, _, _) = get_from(dir.path(), app_spec("published"), "/apps/nothing-here/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_non_public_app_needs_a_session() {
    let dir = app_root("private", &[("index.html", INDEX)]);
    let mut spec = app_spec("published");
    spec["visibility"] = serde_json::json!("project");

    let (status, _, _) = get_from(dir.path(), spec, "/apps/air-quality/").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_modified_asset_is_refused_rather_than_served() {
    let dir = app_root(
        "tampered",
        &[("index.html", INDEX), ("bundle.js", BUNDLE_JS)],
    );
    // The digest manifest is what CI recorded; the file on disk changed afterwards.
    std::fs::write(
        dir.path().join("air-quality/bundle.js"),
        b"console.log('exfiltrate');",
    )
    .expect("tamper");

    let (status, _, body) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/bundle.js",
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !String::from_utf8_lossy(&body).contains("exfiltrate"),
        "the modified bytes must never reach the caller"
    );
}

#[tokio::test]
async fn a_file_without_a_recorded_digest_is_not_part_of_the_bundle() {
    let dir = app_root("stray", &[("index.html", INDEX)]);
    std::fs::write(dir.path().join("air-quality/notes.txt"), b"internal").expect("stray file");

    let (status, _, _) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/notes.txt",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_bundle_without_an_integrity_manifest_serves_nothing() {
    let dir = tempdir::Dir::new("no-manifest");
    let app = dir.path().join("air-quality");
    std::fs::create_dir_all(&app).expect("app dir");
    std::fs::write(app.join("index.html"), INDEX).expect("index");

    let (status, _, _) = get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_traversal_never_reads_outside_the_app_directory() {
    let dir = app_root("traversal", &[("index.html", INDEX)]);
    std::fs::write(dir.path().join("secret.txt"), b"another app's file").expect("neighbour");

    for uri in [
        "/apps/air-quality/../secret.txt",
        "/apps/air-quality/..%2fsecret.txt",
        "/apps/air-quality/%2e%2e/secret.txt",
    ] {
        let (status, _, body) = get_from(dir.path(), app_spec("published"), uri).await;
        assert_ne!(status, StatusCode::OK, "{uri} was served");
        assert!(
            !String::from_utf8_lossy(&body).contains("another app's file"),
            "{uri} read outside the root"
        );
    }
}

#[tokio::test]
async fn an_unknown_path_inside_a_published_app_is_not_the_index() {
    // No SPA fallback: a missing asset stays visible as missing (AP-14).
    let dir = app_root("no-fallback", &[("index.html", INDEX)]);

    let (status, _, body) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/dashboard",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(body, INDEX);
}

async fn page_from(
    root: &std::path::Path,
    spec: serde_json::Value,
    uri: &str,
    accept: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let config = Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        ..Config::for_tests()
    };
    let app = server::app(AppState::new(config, None).with_mirror(mirror_with_app(spec)));
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::ACCEPT, accept)
                .header(header::ACCEPT_LANGUAGE, "sk-SK,sk;q=0.9,en;q=0.8")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn a_browser_on_an_unknown_path_gets_a_page_in_its_language_not_json() {
    // T-4200: a sentence and the way to the App's start, still a 404 and still not the index.
    let dir = app_root("missing-page", &[("index.html", INDEX)]);
    let html = "text/html,application/xhtml+xml,*/*;q=0.8";

    let (status, headers, body) = page_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/no-such-page",
        html,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(body.contains("<html lang=\"sk\">"), "{body}");
    assert!(body.contains("Táto stránka v aplikácii neexistuje."));
    assert!(body.contains("href=\"/apps/air-quality/\""));
    assert!(!body.contains("not found") && !body.contains("\"type\""));

    // An unpublished App answers the very same page: it names no App (AP-18).
    let (status, _, hidden) = page_from(
        dir.path(),
        app_spec("draft"),
        "/apps/air-quality/no-such-page",
        html,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(hidden, body);

    // A script or a fetch asks for no page and keeps the problem body.
    let (status, headers, _) = page_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/assets/missing.js",
        "*/*",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("application/problem+json"));
}

#[tokio::test]
async fn without_an_apps_directory_the_host_answers_not_found() {
    let app = server::app(
        AppState::new(Config::for_tests(), None)
            .with_mirror(mirror_with_app(app_spec("published"))),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/apps/air-quality/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The digest `mirror_with_build` names, as the directory the host fetches it into.
const BUILD_HEX: &str = "a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4";

/// The cache a replica fetches builds into (AP-102), beside the bundles the image ships.
fn cache_of(dir: &tempdir::Dir) -> std::path::PathBuf {
    dir.path().join("cache")
}

/// One fetched build of an app: `{apps_cache_dir}/{name}/{hex}/` with its own integrity manifest
/// (AP-102), beside the bundle the image ships at `{apps_dir}/{name}/`.
fn build_dir(dir: &tempdir::Dir, files: &[(&str, &[u8])]) {
    let app = cache_of(dir).join("air-quality").join(BUILD_HEX);
    std::fs::create_dir_all(&app).expect("build dir");
    let mut digests = serde_json::Map::new();
    for (name, bytes) in files {
        std::fs::write(app.join(name), bytes).expect("bundle file");
        digests.insert((*name).into(), sri_sha384(bytes).into());
    }
    std::fs::write(
        app.join("integrity.json"),
        serde_json::to_vec(&digests).expect("manifest"),
    )
    .expect("write manifest");
}

async fn get_with(root: &std::path::Path, mirror: Arc<Mirror>, uri: &str) -> (StatusCode, Vec<u8>) {
    let config = Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        apps_cache_dir: Some(root.join("cache").to_string_lossy().into_owned()),
        ..Config::for_tests()
    };
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

const NEXT_INDEX: &[u8] = b"<!doctype html><title>air quality, next build</title>";

/// AP-72, AP-102: the host serves the build the manifest names, fetched under its digest, not
/// the bundle the image ships.
#[tokio::test]
async fn the_host_serves_the_build_the_manifest_names() {
    let dir = app_root("named-build", &[("index.html", INDEX)]);
    build_dir(&dir, &[("index.html", NEXT_INDEX)]);

    let (status, body) = get_with(
        dir.path(),
        mirror_with_build(app_spec("published"), "8c56954a1f0e"),
        "/apps/air-quality/",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, NEXT_INDEX, "the named build, not the previous one");
}

/// AP-72: a build this host does not hold keeps the previous one serving, and the app is named
/// as missing its build rather than quietly serving something older as if it were current.
#[tokio::test]
async fn a_build_the_host_does_not_hold_keeps_the_previous_one_serving_and_is_reported() {
    let dir = app_root("missing-build", &[("index.html", INDEX)]);
    let mirror = mirror_with_build(app_spec("published"), "0000000feed");

    let (status, body) = get_with(dir.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX, "the previous publish keeps serving");

    let apps_dir = cache_of(&dir).to_string_lossy().into_owned();
    assert_eq!(
        joinedcontext_portal::apps::static_host::build_missing(Some(&apps_dir), &mirror),
        vec!["air-quality".to_owned()],
        "the app says its build never arrived"
    );

    // The build arrives: nothing else changes and the host follows it.
    build_dir(&dir, &[("index.html", NEXT_INDEX)]);
    let (status, body) = get_with(dir.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, NEXT_INDEX);
    assert!(
        joinedcontext_portal::apps::static_host::build_missing(Some(&apps_dir), &mirror).is_empty(),
        "nothing is missing once the build is there"
    );
}

fn envelope(kind: &str, project: &str, name: &str, spec: serde_json::Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(project.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

/// The `#jc-config` element of a served index, parsed.
fn served_config(body: &[u8]) -> serde_json::Value {
    let html = String::from_utf8(body.to_vec()).expect("an html index");
    let open = "<script id=\"jc-config\" type=\"application/json\">";
    let start = html.find(open).expect("the index carries #jc-config") + open.len();
    let end = start + html[start..].find("</script>").expect("the element closes");
    assert_eq!(
        html.matches("id=\"jc-config\"").count(),
        1,
        "exactly one configuration: {html}"
    );
    serde_json::from_str(&html[start..end]).expect("the configuration is JSON")
}

/// T-2667, AP-04: once the App's grants are committed it reads through its own endpoint alone.
/// Offering every endpoint of the space made the SDK refuse a type several of them serve
/// ("served by more than one endpoint"), and the bikes sample showed no station on dev.
#[tokio::test]
async fn an_app_with_its_own_endpoint_is_served_that_endpoint_alone() {
    let index: &[u8] = b"<!doctype html><html><head><title>kpi</title></head><body></body></html>";
    let dir = app_root("config", &[("index.html", index)]);
    let mut spec = app_spec("published");
    spec["dataNeeds"] = serde_json::json!([{
        "contextSpaceRef": { "kind": "ContextSpace", "name": "bbsk-kpi" },
        "types": ["KeyPerformanceIndicator"],
        "operations": ["queryEntity"]
    }]);
    let mirror = mirror_with_app(spec);
    let endpoint =
        |space: &str, slug: &str| serde_json::json!({ "contextSpaceRef": space, "slug": slug });
    for (name, slug) in [
        ("bbsk-kpi", "regionslug"),
        ("bbsk-kpi-public", "publicslug"),
    ] {
        mirror.upsert(envelope(
            "Endpoint",
            "ovzdusie",
            name,
            endpoint("bbsk-kpi", slug),
        ));
    }
    // A hand-written endpoint with the app's endpoint name is not the app's (T-2632).
    let (_, body) = get_with(dir.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(
        served_config(&body)["endpoints"].as_array().map(Vec::len),
        Some(2)
    );

    let mut own = envelope(
        "Endpoint",
        "ovzdusie",
        "app-air-quality",
        endpoint("bbsk-kpi", "ownslug"),
    );
    own.metadata.annotations.insert(
        "joinedcontext.com/generated-by".into(),
        joinedcontext_portal::apps::reconciler::GENERATOR.into(),
    );
    mirror.upsert(own);
    let (status, body) = get_with(dir.path(), mirror, "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    let config = served_config(&body);
    assert_eq!(config["slug"], "ownslug");
    assert_eq!(config["endpointName"], "app-air-quality");
    let slugs: Vec<&str> = config["endpoints"]
        .as_array()
        .expect("endpoints")
        .iter()
        .filter_map(|e| e["slug"].as_str())
        .collect();
    assert_eq!(slugs, ["ownslug"]);
}

/// AP-04 (T-2933): a further space is read through its public endpoints alone, after the App's
/// own; another endpoint of that space is not offered, so the App's client never names it.
#[tokio::test]
async fn a_further_space_is_served_through_its_public_endpoint_alone() {
    let index: &[u8] = b"<!doctype html><html><head><title>kpi</title></head><body></body></html>";
    let dir = app_root("further", &[("index.html", index)]);
    let mut spec = app_spec("published");
    spec["dataNeeds"] = serde_json::json!([
        {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "bbsk-kpi" },
            "types": ["KeyPerformanceIndicator"],
            "operations": ["queryEntity"]
        },
        {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "bbsk-registre" },
            "types": ["AdministrativeArea"],
            "operations": ["queryEntity"]
        }
    ]);
    let mirror = mirror_with_app(spec);
    for (name, space, slug, audience) in [
        ("bbsk-kpi", "bbsk-kpi", "regionslug", "project-list"),
        ("bbsk-registre", "bbsk-registre", "registreslug", "public"),
        (
            "bbsk-registre-steward",
            "bbsk-registre",
            "stewardslug",
            "organization",
        ),
    ] {
        mirror.upsert(envelope(
            "Endpoint",
            "ovzdusie",
            name,
            serde_json::json!({ "contextSpaceRef": space, "slug": slug, "audience": audience }),
        ));
    }

    let (status, body) = get_with(dir.path(), mirror, "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    let config = served_config(&body);
    assert_eq!(
        config["slug"], "regionslug",
        "the own space's endpoint is the primary"
    );
    let slugs: Vec<&str> = config["endpoints"]
        .as_array()
        .expect("endpoints")
        .iter()
        .filter_map(|e| e["slug"].as_str())
        .collect();
    assert_eq!(slugs, ["regionslug", "registreslug"]);
}

/// T-2457: the region's application reads its own indicators and the city's through the shared
/// reference, so the index it is served names both endpoints, by space, and never a token.
#[tokio::test]
async fn the_served_index_names_the_apps_endpoint_and_the_shared_one() {
    let index: &[u8] = b"<!doctype html><html><head><title>kpi</title></head><body></body></html>";
    let dir = app_root("config", &[("index.html", index)]);
    let mut spec = app_spec("published");
    spec["dataNeeds"] = serde_json::json!([{
        "contextSpaceRef": { "kind": "ContextSpace", "name": "bbsk-kpi" },
        "types": ["KeyPerformanceIndicator"],
        "operations": ["queryEntity"]
    }]);

    let mirror = mirror_with_app(spec);
    let endpoint =
        |space: &str, slug: &str| serde_json::json!({ "contextSpaceRef": space, "slug": slug });
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie",
        "bbsk-kpi",
        endpoint("bbsk-kpi", "regionslug"),
    ));
    // Another space of the same project: not a need of the app, so not in its configuration.
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie",
        "bbsk-kraj",
        endpoint("bbsk-kraj", "rawslug"),
    ));
    mirror.upsert(envelope(
        "Endpoint",
        "banskabystrica",
        "banskabystrica-kpi",
        endpoint("banskabystrica-kpi", "cityslug"),
    ));
    mirror.upsert(envelope(
        "SharedSpaceReference",
        "ovzdusie",
        "mesto-kpi",
        serde_json::json!({
            "alias": "mesto-kpi",
            "endpointRef": { "project": "banskabystrica", "name": "banskabystrica-kpi" }
        }),
    ));
    // A reference to an endpoint that is not there adds nothing and breaks nothing.
    mirror.upsert(envelope(
        "SharedSpaceReference",
        "ovzdusie",
        "gone",
        serde_json::json!({ "alias": "gone", "endpointRef": { "project": "nowhere", "name": "x" } }),
    ));

    let (status, body) = get_with(dir.path(), mirror, "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    let config = served_config(&body);
    assert_eq!(config["transport"], "origin");
    assert_eq!(config["appName"], "air-quality");
    assert_eq!(
        config["slug"], "regionslug",
        "the app's own endpoint is the primary"
    );
    assert_eq!(config["space"], "bbsk-kpi");
    assert_eq!(config["endpointName"], "bbsk-kpi");
    let endpoints: Vec<(String, String)> = config["endpoints"]
        .as_array()
        .expect("endpoints")
        .iter()
        .map(|e| {
            (
                e["slug"].as_str().unwrap().into(),
                e["space"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        endpoints,
        vec![
            ("regionslug".to_owned(), "bbsk-kpi".to_owned()),
            ("cityslug".to_owned(), "banskabystrica-kpi".to_owned()),
        ]
    );
    assert_eq!(
        config["endpoints"][0]["types"],
        serde_json::json!(["KeyPerformanceIndicator"])
    );
    let text = config.to_string().to_lowercase();
    assert!(
        !text.contains("token") && !text.contains("bearer"),
        "{config}"
    );
}

/// T-3599: the loader stores a reference by the slug it resolved (`endpointSlug`), as dev holds
/// helsinki-mobility's `city-bikes`; the index names the shared endpoint all the same, and a slug
/// no endpoint carries adds nothing.
#[tokio::test]
async fn a_reference_stored_by_slug_names_the_shared_endpoint() {
    let dir = app_root("byslug", &[("index.html", INDEX)]);
    let mut spec = app_spec("published");
    spec["dataNeeds"] = serde_json::json!([{
        "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" },
        "types": ["BikeHireDockingStation"],
        "operations": ["queryEntity"]
    }]);
    let mirror = mirror_with_app(spec);
    let endpoint =
        |space: &str, slug: &str| serde_json::json!({ "contextSpaceRef": space, "slug": slug });
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie",
        "mobility",
        endpoint("mobility", "ownslug"),
    ));
    mirror.upsert(envelope(
        "Endpoint",
        "helsinki",
        "helsinki-bikes",
        endpoint("helsinki-bikes", "bikesslug"),
    ));
    for (name, slug) in [("city-bikes", "bikesslug"), ("gone", "noslug")] {
        mirror.upsert(envelope(
            "SharedSpaceReference",
            "ovzdusie",
            name,
            serde_json::json!({ "alias": name, "endpointSlug": slug }),
        ));
    }

    let (status, body) = get_with(dir.path(), mirror, "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    let config = served_config(&body);
    let slugs: Vec<&str> = config["endpoints"]
        .as_array()
        .expect("endpoints")
        .iter()
        .filter_map(|e| e["slug"].as_str())
        .collect();
    assert_eq!(slugs, ["ownslug", "bikesslug"], "{config}");
    assert_eq!(config["endpoints"][1]["space"], "helsinki-bikes");
}

/// An index that reads nothing is served byte for byte as it was built and signed.
#[tokio::test]
async fn an_app_without_data_needs_is_served_as_built() {
    let dir = app_root("asbuilt", &[("index.html", INDEX)]);
    let (status, _, body) = get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX);
}

/// One request for an app with the `Host` a browser sends, on a Portal whose apps live under
/// `https://example.org`, each on `{name}.apps.example.org`, and whose own UI lives on
/// `https://portal.example.org`.
async fn get_on_host(root: &std::path::Path, host: &str, uri: &str) -> axum::response::Response {
    let config = Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        public_base_url: "https://portal.example.org".parse().unwrap(),
        apps_url: Some("https://example.org".parse().unwrap()),
        ..Config::for_tests()
    };
    let app = server::app(
        AppState::new(config, None).with_mirror(mirror_with_app(app_spec("published"))),
    );
    app.oneshot(
        Request::builder()
            .uri(uri)
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

/// AP-122: on its own origin an App may be framed by the Portal, which opens it under its header,
/// and by nobody else; no `X-Frame-Options` refuses the Portal's frame.
#[tokio::test]
async fn on_the_apps_origin_only_the_portal_frames_an_app() {
    let dir = app_root("framed", &[("index.html", INDEX)]);
    let response = get_on_host(
        dir.path(),
        "air-quality.apps.example.org",
        "/apps/air-quality/",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(
        csp.ends_with("frame-ancestors https://portal.example.org"),
        "{csp}"
    );
    assert!(response.headers().get(header::X_FRAME_OPTIONS).is_none());
}

/// T-2476: an app served on the Portal's own origin would reach the Portal API with the
/// viewer's session cookie and read the CSRF cookie. The Portal host never serves one; it
/// sends the browser to the same path on the apps origin.
#[tokio::test]
async fn an_app_asked_for_on_the_portal_host_is_sent_to_the_apps_origin() {
    let dir = app_root(
        "portal-host",
        &[("index.html", INDEX), ("bundle.js", BUNDLE_JS)],
    );

    for (uri, location) in [
        (
            "/apps/air-quality/",
            "https://air-quality.apps.example.org/",
        ),
        (
            "/apps/air-quality/bundle.js?v=2",
            "https://air-quality.apps.example.org/bundle.js?v=2",
        ),
    ] {
        let response = get_on_host(dir.path(), "portal.example.org", uri).await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{uri}");
        assert_eq!(response.headers()[header::LOCATION], location, "{uri}");
        assert!(response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_none());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            body.is_empty(),
            "no byte of the bundle on the Portal origin"
        );
    }

    // Any host but the App's own is the same refusal: the apex, another App's host (whose page
    // would then share this App's storage, AP-133), a look-alike. A Host header is the caller's
    // to set, and the target is never read from it.
    for host in [
        "evil.example.net",
        "example.org",
        "hsl-transport.apps.example.org",
        "air-quality.apps.example.org.evil.net",
        "apps.example.org",
    ] {
        let response = get_on_host(dir.path(), host, "/apps/air-quality/").await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{host}");
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://air-quality.apps.example.org/",
            "{host}"
        );
    }
}

#[tokio::test]
async fn an_app_is_served_on_the_apps_origin() {
    let dir = app_root("apps-host", &[("index.html", INDEX)]);

    for host in [
        "air-quality.apps.example.org",
        "AIR-QUALITY.apps.EXAMPLE.org:443",
    ] {
        let response = get_on_host(dir.path(), host, "/apps/air-quality/").await;
        assert_eq!(response.status(), StatusCode::OK, "{host}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, INDEX);
    }
    let response = get_on_host(
        dir.path(),
        "air-quality.apps.example.org:8443",
        "/apps/air-quality/",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::PERMANENT_REDIRECT,
        "another port is another origin"
    );
}

/// AP-84: a signed-in person's first index on the apps origin brings the double-submit CSRF
/// cookie a function call needs there; the Portal's own is host-only on the Portal host. An
/// anonymous visitor gets none, and one who already holds it keeps the one they have.
#[tokio::test]
async fn a_signed_in_index_brings_the_apps_origins_csrf_cookie() {
    let dir = app_root("csrf-cookie", &[("index.html", INDEX)]);
    let config = edge_config(dir.path());
    let token = common::REALM.person_token("air-quality", "jana", &[]);
    let app = server::app(
        AppState::new(config, None).with_mirror(mirror_with_app(app_spec("published"))),
    );
    let set_cookies = |signed_in: bool, cookie: Option<&str>| {
        let app = app.clone();
        let cookie = cookie.map(str::to_owned);
        let token = signed_in.then(|| token.clone());
        async move {
            let mut request = Request::builder().uri("/apps/air-quality/");
            if let Some(token) = token {
                request = request.header("x-access-token", token);
            }
            if let Some(cookie) = cookie {
                request = request.header(header::COOKIE, cookie);
            }
            let response = app
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        }
    };

    let csrf_cookie = |signed_in: bool, cookie: Option<&'static str>| {
        let lines = set_cookies(signed_in, cookie);
        async move {
            lines
                .await
                .into_iter()
                .find(|value| value.starts_with("__Host-jc_csrf="))
        }
    };

    // T-4395: an App built before the prefix reads `jc_csrf`; it gets the same token, and only
    // the `__Host-` one is ever compared (tests/auth_tests.rs).
    let lines = set_cookies(true, None).await;
    let value = |name: &str| {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .and_then(|l| l.split(';').next())
            .map(str::to_owned)
    };
    assert!(value("__Host-jc_csrf").is_some(), "{lines:?}");
    assert_eq!(value("jc_csrf"), value("__Host-jc_csrf"), "{lines:?}");
    let issued = csrf_cookie(true, None)
        .await
        .expect("a CSRF cookie for the signed-in person");
    assert!(
        issued.contains("Secure") && issued.contains("SameSite=Lax") && issued.contains("Path=/"),
        "{issued}"
    );
    assert_eq!(
        csrf_cookie(false, None).await,
        None,
        "nothing for an anonymous visitor"
    );
    assert_eq!(
        csrf_cookie(true, Some("__Host-jc_csrf=already-held")).await,
        None,
        "a held token is kept"
    );
}

/// A Portal behind the edge, trusting the process's realm, serving the apps under `root`.
fn edge_config(root: &std::path::Path) -> Config {
    let realm = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("a realm");
    Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        trust_edge_token: true,
        ..realm
    }
}

/// One request for the app with the edge's `token`, or anonymously.
async fn get_as(
    root: &std::path::Path,
    mirror: Arc<Mirror>,
    token: Option<String>,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let config = edge_config(root);
    let mut request = Request::builder()
        .uri(uri)
        .header(header::ACCEPT_LANGUAGE, "en");
    if let Some(token) = token {
        request = request.header("x-access-token", token);
    }
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body.to_vec())
}

/// A `visibility: roles` app reading one space, with a viewer group and one steward.
fn roles_app() -> Arc<Mirror> {
    let mut spec = app_spec("published");
    spec["visibility"] = serde_json::json!("roles");
    spec["roles"] = serde_json::json!([
        { "name": "viewer", "title": { "en": "Viewer" } },
        { "name": "steward", "title": { "en": "Steward" } }
    ]);
    spec["access"] = serde_json::json!([
        { "role": "viewer", "subjects": [{ "group": "ovzdusie-operations" }] },
        { "role": "steward", "subjects": [{ "user": "jana" }] }
    ]);
    spec["dataNeeds"] = serde_json::json!([{
        "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
        "types": ["AirQualityObserved"],
        "operations": ["queryEntity"]
    }]);
    let mirror = mirror_with_app(spec);
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie",
        "app-air-quality",
        serde_json::json!({ "contextSpaceRef": "ovzdusie", "slug": "appslug" }),
    ));
    mirror
}

/// AP-93: a signed-in person holding no role of a `visibility: roles` app gets the 403 page that
/// names the roles and no member; an anonymous visitor gets it too, and neither sees a byte of
/// the bundle, the index or an asset.
#[tokio::test]
async fn a_person_without_a_role_is_refused_with_the_roles_and_no_member() {
    let dir = app_root(
        "roles-refused",
        &[("index.html", INDEX), ("app.js", BUNDLE_JS)],
    );
    // Petra holds no role of this App; the Portal's own login and another App's token are not
    // this App's, whatever roles they carry (AP-92).
    let petra = common::REALM.person_token("air-quality", "petra", &[]);
    let portal = common::REALM.person_token_of("portal-api", "portal-api", "jana", &["steward"]);
    let other = common::REALM.person_token("other", "jana", &["steward"]);
    for (who, uri) in [
        (Some(petra.clone()), "/apps/air-quality/"),
        (Some(petra), "/apps/air-quality/app.js"),
        (Some(portal), "/apps/air-quality/"),
        (Some(other), "/apps/air-quality/"),
        (None, "/apps/air-quality/"),
    ] {
        let (status, headers, body) = get_as(dir.path(), roles_app(), who, uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
        let page = String::from_utf8(body).expect("html");
        assert!(
            page.contains("<li>Viewer</li>") && page.contains("<li>Steward</li>"),
            "{page}"
        );
        assert!(page.contains("project ovzdusie"), "{page}");
        assert!(
            !page.contains("jana") && !page.contains("ovzdusie-operations"),
            "{page}"
        );
        assert!(
            !page.contains("air quality</title>") && !page.contains("console.log"),
            "{page}"
        );
    }
}

/// AP-95, SDK-35: a member is served the index with their roles in this app, the platform's never,
/// and an index that carries a person is kept by no cache.
#[tokio::test]
async fn a_member_is_served_the_index_with_their_roles_and_no_store() {
    let dir = app_root("roles-served", &[("index.html", INDEX)]);
    // The token's realm roles and other clients' roles are left out; its roles of this App's
    // client come in the order the manifest declares them.
    let jana = common::REALM.person_token("air-quality", "jana", &["steward", "viewer"]);
    let (status, headers, body) =
        get_as(dir.path(), roles_app(), Some(jana), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(
        served_config(&body)["user"],
        serde_json::json!({
            "id": "sub-jana",
            "name": "jana@hel.fi",
            "email": "jana@hel.fi",
            "roles": ["viewer", "steward"]
        })
    );
}

/// AP-95: an anonymous visitor of a public app is served `user: null`, and the index stays
/// revalidated rather than private, since it carries nobody.
#[tokio::test]
async fn an_anonymous_visitor_is_served_no_user() {
    let dir = app_root("roles-anonymous", &[("index.html", INDEX)]);
    let mirror = roles_app();
    let mut envelope = mirror
        .get("ovzdusie", "App", "air-quality")
        .expect("the app");
    envelope.spec["visibility"] = serde_json::json!("public");
    mirror.upsert(envelope);
    let (status, headers, body) = get_as(dir.path(), mirror, None, "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    assert_eq!(served_config(&body)["user"], serde_json::Value::Null);
}

/// AP-67, AP-12, T-2833: a published App's map reads the platform's basemap. The index names the
/// style URL in `#jc-config`, and the policy admits that project's basemap route in `connect-src`
/// and `img-src` and nothing more; without a configured basemap neither appears.
#[tokio::test]
async fn a_configured_basemap_is_in_the_config_and_the_policy_admits_only_its_route() {
    use joinedcontext_portal::config::BasemapConfig;

    let index: &[u8] = b"<!doctype html><html><head><title>map</title></head><body></body></html>";
    let dir = app_root("basemap", &[("index.html", index)]);
    let mut spec = app_spec("published");
    spec["dataNeeds"] = serde_json::json!([{
        "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
        "types": ["AirQualityObserved"],
        "operations": ["queryEntity"]
    }]);
    let mirror = mirror_with_app(spec);
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie",
        "air-public",
        serde_json::json!({ "contextSpaceRef": "air", "slug": "airslug" }),
    ));

    let get = |basemap: Option<BasemapConfig>| {
        let mut config = Config {
            apps_dir: Some(dir.path().to_string_lossy().into_owned()),
            ..Config::for_tests()
        };
        config.basemap = basemap;
        let state = AppState::new(config, None).with_mirror(mirror.clone());
        async move {
            let response = server::app(state)
                .oneshot(
                    Request::builder()
                        .uri("/apps/air-quality/")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .to_owned();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            (csp, served_config(&body))
        }
    };

    let (csp, config) = get(None).await;
    assert!(config.get("basemap").is_none(), "{config}");
    assert!(!csp.contains("/basemap/"), "{csp}");

    let basemap = BasemapConfig::for_tests(
        "https://tile.openstreetmap.org/{z}/{x}/{y}.png".to_owned(),
        "© OpenStreetMap contributors".to_owned(),
        dir.path().join("tiles"),
    );
    let (csp, config) = get(Some(basemap)).await;
    let base = Config::for_tests().public_base_url.to_string();
    let prefix = format!(
        "{}/api/v1/projects/ovzdusie/basemap/",
        base.trim_end_matches('/')
    );
    assert_eq!(config["basemap"], format!("{prefix}default/style.json"));
    assert!(
        csp.contains(&format!("connect-src 'self' {prefix};")),
        "{csp}"
    );
    assert!(
        csp.contains(&format!("img-src 'self' data: blob: {prefix};")),
        "{csp}"
    );
    assert!(
        !csp.contains("openstreetmap"),
        "the tile host stays behind the Portal: {csp}"
    );
}

/// The app as the sync writes it: `Live`, and no build written back.
fn mirror_live_without_build(spec: serde_json::Value) -> Arc<Mirror> {
    let mirror = mirror_with_app(spec);
    let mut envelope = mirror
        .get("ovzdusie", "App", "air-quality")
        .expect("the app");
    envelope.status = Some(joinedcontext_portal::resource::Status {
        phase: joinedcontext_portal::resource::Phase::Live,
        observed_revision: None,
        source_url: None,
        conditions: Vec::new(),
        build: None,
        shard: None,
        domain_verification: None,
        jobs: Vec::new(),
        notifications: None,
    });
    mirror.upsert(envelope);
    mirror
}

fn app_status(mirror: &Mirror) -> joinedcontext_portal::resource::Status {
    mirror
        .get("ovzdusie", "App", "air-quality")
        .and_then(|env| env.status)
        .expect("a status")
}

/// T-2989, AP-13a: a published App with no build and no bundle the image ships answers 404 on
/// its host, so it reads Pending with a red Ready naming why, never Live.
#[tokio::test]
async fn a_published_app_without_a_build_reads_pending_not_live() {
    let empty = tempdir::Dir::new("no-build");
    let mirror = mirror_live_without_build(app_spec("published"));
    let (status, _) = get_with(empty.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the host has nothing to serve"
    );

    let apps_dir = empty.path().to_string_lossy().into_owned();
    assert_eq!(
        joinedcontext_portal::apps::static_host::report_unbuilt(
            Some(&apps_dir),
            &mirror,
            &Transitions::default()
        ),
        vec!["air-quality".to_owned()]
    );
    let written = app_status(&mirror);
    assert_eq!(
        written.phase,
        joinedcontext_portal::resource::Phase::Pending
    );
    let [ready] = written.conditions.as_slice() else {
        panic!("one condition: {:?}", written.conditions);
    };
    assert_eq!(
        (
            ready.r#type.as_str(),
            ready.status.as_str(),
            ready.reason.as_deref()
        ),
        ("Ready", "False", Some("NoBuild"))
    );
    assert!(ready
        .message
        .as_deref()
        .is_some_and(|m| m.contains("rebuild")));
}

/// The rule's edges: a bundle the image ships is served, a named build is 6b's to judge, and an
/// App that is not published serves nothing by design; none of them is touched.
#[tokio::test]
async fn only_a_published_app_with_nothing_to_serve_is_marked() {
    let shipped = app_root("shipped-no-build", &[("index.html", INDEX)]);
    let apps_dir = shipped.path().to_string_lossy().into_owned();
    let mirror = mirror_live_without_build(app_spec("published"));
    let (status, _) = get_with(shipped.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK, "the shipped bundle serves");
    assert!(joinedcontext_portal::apps::static_host::report_unbuilt(
        Some(&apps_dir),
        &mirror,
        &Transitions::default()
    )
    .is_empty());
    assert_eq!(
        app_status(&mirror).phase,
        joinedcontext_portal::resource::Phase::Live
    );

    let empty = tempdir::Dir::new("built-or-draft");
    let apps_dir = empty.path().to_string_lossy().into_owned();
    for mirror in [
        mirror_with_build(app_spec("published"), "8c56954a1f0e"),
        mirror_live_without_build(app_spec("draft")),
    ] {
        assert!(joinedcontext_portal::apps::static_host::report_unbuilt(
            Some(&apps_dir),
            &mirror,
            &Transitions::default()
        )
        .is_empty());
        assert_eq!(
            app_status(&mirror).phase,
            joinedcontext_portal::resource::Phase::Live
        );
    }
}

/// T-2989: run after run the App still has no build, so its `Ready` condition keeps the time
/// the first run said it; nothing transitioned.
#[tokio::test]
async fn a_second_run_keeps_the_time_the_app_first_had_no_build() {
    let empty = tempdir::Dir::new("no-build-twice");
    let apps_dir = empty.path().to_string_lossy().into_owned();
    let mirror = mirror_live_without_build(app_spec("published"));
    joinedcontext_portal::apps::static_host::report_unbuilt(
        Some(&apps_dir),
        &mirror,
        &Transitions::default(),
    );
    let first = app_status(&mirror).conditions[0].last_transition_time;
    assert!(first.is_some());

    // The next run: the sync writes Live with no conditions, then the step says it again.
    let before = Transitions::of(&mirror);
    let next = mirror_live_without_build(app_spec("published"));
    std::thread::sleep(std::time::Duration::from_millis(5));
    joinedcontext_portal::apps::static_host::report_unbuilt(Some(&apps_dir), &next, &before);
    let again = app_status(&next);
    assert_eq!(again.phase, joinedcontext_portal::resource::Phase::Pending);
    assert_eq!(again.conditions[0].last_transition_time, first);
}

/// T-4910: who opens an App of each visibility. Olga created it and holds a binding in another
/// project, Marek is a member of its project, Eva of another project of the organization, Jana holds
/// its viewer role and no binding, Pavol is a realm user nobody bound. A refusal is the `404` of a
/// draft, byte for byte, except the `403` page of a `roles` App (AP-18, AP-93).
#[tokio::test]
async fn each_visibility_opens_for_its_own_callers_alone() {
    let dir = app_root(
        "visibility-matrix",
        &[("index.html", INDEX), ("app.js", BUNDLE_JS)],
    );
    let token =
        |who: &str, roles: &[&str]| Some(common::REALM.person_token("air-quality", who, roles));
    let callers = [
        ("anonymous", None),
        ("pavol", token("pavol", &[])),
        ("eva", token("eva", &[])),
        ("marek", token("marek", &[])),
        ("olga", token("olga", &[])),
        ("jana", token("jana", &["viewer"])),
    ];
    // In the order of `callers`; 403 is the roles page.
    let table = [
        ("public", [200, 200, 200, 200, 200, 200]),
        ("organization", [404, 404, 200, 200, 200, 404]),
        ("project", [404, 404, 404, 200, 404, 404]),
        ("private", [404, 404, 404, 404, 200, 404]),
        ("roles", [403, 403, 403, 403, 403, 200]),
    ];
    for (visibility, expected) in table {
        let mirror = roles_app();
        let mut app = mirror
            .get("ovzdusie", "App", "air-quality")
            .expect("the app");
        app.spec["visibility"] = serde_json::json!(visibility);
        app.metadata
            .annotations
            .insert("joinedcontext.com/owner".into(), "sub-olga".into());
        mirror.upsert(app);
        common::bind(&mirror, "marek", "ovzdusie");
        common::bind(&mirror, "eva", "doprava");
        common::bind(&mirror, "olga", "doprava");
        for ((caller, token), expected) in callers.iter().zip(expected) {
            for uri in ["/apps/air-quality/", "/apps/air-quality/app.js"] {
                let (status, _, body) =
                    get_as(dir.path(), mirror.clone(), token.clone(), uri).await;
                assert_eq!(status.as_u16(), expected, "{caller} on {visibility} {uri}");
                if expected == 404 {
                    // The answer for a draft of the same name: nothing tells the two apart.
                    let draft = roles_app();
                    let mut app = draft
                        .get("ovzdusie", "App", "air-quality")
                        .expect("the app");
                    app.spec["lifecycle"] = serde_json::json!("draft");
                    draft.upsert(app);
                    let (_, _, missing) = get_as(dir.path(), draft, token.clone(), uri).await;
                    assert_eq!(body, missing, "{caller} on {visibility} {uri}");
                }
            }
        }
    }
}

/// F-BNT3-009: a private App stored without an owner (written before T-4910) opens for the
/// administrators of its project alone. Two app-editors of the project, who may propose an App
/// but not approve one, get the `404` a stranger gets; the administrator, who can give it an
/// owner, opens it.
#[tokio::test]
async fn an_ownerless_private_app_opens_for_its_projects_administrators_alone() {
    let dir = app_root("ownerless-private", &[("index.html", INDEX)]);
    let mirror = roles_app();
    let mut app = mirror
        .get("ovzdusie", "App", "air-quality")
        .expect("the app");
    app.spec["visibility"] = serde_json::json!("private");
    assert!(!app
        .metadata
        .annotations
        .contains_key("joinedcontext.com/owner"));
    mirror.upsert(app);
    let org = joinedcontext_portal::permissions::ORG_NAMESPACE;
    for (role, verbs) in [
        ("app-editors", serde_json::json!(["read", "propose"])),
        (
            "app-admins",
            serde_json::json!(["read", "propose", "approve"]),
        ),
    ] {
        mirror.upsert(common::envelope(
            "Role",
            role,
            org,
            serde_json::json!({ "rules": [{ "kinds": ["App"], "verbs": verbs }] }),
        ));
    }
    for (who, role) in [
        ("marek", "app-editors"),
        ("eva", "app-editors"),
        ("olga", "app-admins"),
    ] {
        mirror.upsert(common::envelope(
            "RoleBinding",
            &format!("{who}-in-ovzdusie"),
            org,
            serde_json::json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role,
                                "scope": { "project": "ovzdusie" } }),
        ));
    }
    for (who, expected) in [("marek", 404), ("eva", 404), ("olga", 200)] {
        let token = Some(common::REALM.person_token("air-quality", who, &[]));
        let (status, _, _) = get_as(dir.path(), mirror.clone(), token, "/apps/air-quality/").await;
        assert_eq!(status.as_u16(), expected, "{who}");
    }
}

/// T-4922: the edge asks `/edge/apps/{name}/admit` before a pod App's server or a wasm App's shard
/// answers, and gets the T-4910 matrix: `204` where the static host would serve the page, else the
/// static host's own refusal, the `404` of a draft byte for byte or the `403` page of a `roles` App.
#[tokio::test]
async fn the_edge_admits_each_visibility_of_a_pod_or_wasm_app_for_its_own_callers_alone() {
    let dir = app_root("visibility-admit", &[("index.html", INDEX)]);
    let token =
        |who: &str, roles: &[&str]| Some(common::REALM.person_token("air-quality", who, roles));
    let callers = [
        ("anonymous", None),
        ("pavol", token("pavol", &[])),
        ("eva", token("eva", &[])),
        ("marek", token("marek", &[])),
        ("olga", token("olga", &[])),
        ("jana", token("jana", &["viewer"])),
    ];
    let table = [
        ("public", [204, 204, 204, 204, 204, 204]),
        ("organization", [404, 404, 204, 204, 204, 404]),
        ("project", [404, 404, 404, 204, 404, 404]),
        ("private", [404, 404, 404, 404, 204, 404]),
        ("roles", [403, 403, 403, 403, 403, 204]),
    ];
    let uri = "/edge/apps/air-quality/admit";
    for class in ["ui-rust", "wasm"] {
        for (visibility, expected) in table {
            let mirror = roles_app();
            let mut app = mirror
                .get("ovzdusie", "App", "air-quality")
                .expect("the app");
            app.spec["kind"] = serde_json::json!(class);
            app.spec["visibility"] = serde_json::json!(visibility);
            app.metadata
                .annotations
                .insert("joinedcontext.com/owner".into(), "sub-olga".into());
            mirror.upsert(app.clone());
            common::bind(&mirror, "marek", "ovzdusie");
            common::bind(&mirror, "eva", "doprava");
            common::bind(&mirror, "olga", "doprava");
            let draft = roles_app();
            app.spec["lifecycle"] = serde_json::json!("draft");
            draft.upsert(app);
            for ((caller, token), expected) in callers.iter().zip(expected) {
                let (status, _, body) =
                    get_as(dir.path(), mirror.clone(), token.clone(), uri).await;
                let case = format!("{caller} on {class} {visibility}");
                assert_eq!(status.as_u16(), expected, "{case}");
                match expected {
                    204 => assert!(body.is_empty(), "{case}"),
                    404 => {
                        let (_, _, missing) =
                            get_as(dir.path(), draft.clone(), token.clone(), uri).await;
                        assert_eq!(body, missing, "{case}");
                    }
                    _ => assert!(String::from_utf8_lossy(&body).contains("Viewer"), "{case}"),
                }
            }
        }
    }
}

/// One request to a Portal that lives across the test, as the edge forwards it.
async fn ask_held(
    state: &AppState,
    token: Option<&str>,
    uri: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .uri(uri)
        .header(header::ACCEPT_LANGUAGE, "en");
    if let Some(token) = token {
        request = request.header("x-access-token", token);
    }
    if let Some(etag) = if_none_match {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body.to_vec())
}

const HASHED_JS: &str = "/apps/air-quality/assets/index-B4tq81Zr.js";

fn held_app(case: &str) -> tempdir::Dir {
    app_root(
        case,
        &[
            ("index.html", INDEX),
            ("bundle.js", BUNDLE_JS),
            ("assets/index-B4tq81Zr.js", BUNDLE_JS),
        ],
    )
}

/// T-4914: the cache headers per visibility. A non-public App's index is kept by nobody, its
/// hashed files a minute and its other files not without asking; every answer varies on the
/// edge's session cookie. A public App's hashed files are kept for good. No answer is `public`.
#[tokio::test]
async fn each_visibility_caches_for_its_readers_alone() {
    let dir = held_app("cache-per-visibility");
    let olga = common::REALM.person_token("air-quality", "olga", &["viewer"]);
    let table = [
        (
            "public",
            [
                "private, no-store",
                "no-cache",
                "private, max-age=31536000, immutable",
            ],
        ),
        (
            "organization",
            [
                "private, no-store",
                "private, no-cache",
                "private, max-age=60",
            ],
        ),
        (
            "project",
            [
                "private, no-store",
                "private, no-cache",
                "private, max-age=60",
            ],
        ),
        (
            "private",
            [
                "private, no-store",
                "private, no-cache",
                "private, max-age=60",
            ],
        ),
        (
            "roles",
            [
                "private, no-store",
                "private, no-cache",
                "private, max-age=60",
            ],
        ),
    ];
    for (visibility, expected) in table {
        let mirror = roles_app();
        let mut app = mirror
            .get("ovzdusie", "App", "air-quality")
            .expect("the app");
        app.spec["visibility"] = serde_json::json!(visibility);
        app.metadata
            .annotations
            .insert("joinedcontext.com/owner".into(), "sub-olga".into());
        mirror.upsert(app);
        common::bind(&mirror, "olga", "ovzdusie");
        let state = AppState::new(edge_config(dir.path()), None).with_mirror(mirror);
        let uris = [
            "/apps/air-quality/",
            "/apps/air-quality/bundle.js",
            HASHED_JS,
        ];
        for (uri, cache) in uris.into_iter().zip(expected) {
            let (status, headers, _) = ask_held(&state, Some(&olga), uri, None).await;
            assert_eq!(status, StatusCode::OK, "{visibility} {uri}");
            assert_eq!(headers[header::CACHE_CONTROL], cache, "{visibility} {uri}");
            // A public page is the same for every reader; only the compression varies (T-5598).
            let vary: Vec<&str> = headers
                .get_all(header::VARY)
                .iter()
                .flat_map(|v| v.to_str().unwrap().split(','))
                .map(str::trim)
                .collect();
            assert_eq!(
                vary.iter().any(|v| v.eq_ignore_ascii_case("cookie")),
                visibility != "public",
                "{visibility} {uri}: {vary:?}"
            );
            // A file is revalidated by its digest; the index is the reader's own and has none.
            assert_eq!(
                headers.contains_key(header::ETAG),
                uri != "/apps/air-quality/",
                "{visibility} {uri}"
            );
        }
    }
}

/// T-4914: each way of ending a reader's access takes effect on the reader's next request, a
/// revalidation included, and a retired App answers every file `404`. The clock
/// measures each step against the one-minute bound the edge's token lifespan sets.
#[tokio::test]
async fn revoking_access_ends_it_at_the_next_request() {
    let dir = held_app("revocation");
    let mirror = roles_app();
    let set = |field: &str, value: &str| {
        let mut app = mirror
            .get("ovzdusie", "App", "air-quality")
            .expect("the app");
        app.spec[field] = serde_json::json!(value);
        mirror.upsert(app);
    };
    set("visibility", "project");
    common::bind(&mirror, "marek", "ovzdusie");
    common::bind(&mirror, "eva", "doprava");
    common::bind(&mirror, "olga", "ovzdusie");
    let state = AppState::new(edge_config(dir.path()), None).with_mirror(mirror.clone());
    let token = |who: &str| common::REALM.person_token("air-quality", who, &[]);
    let (marek, eva, olga) = (token("marek"), token("eva"), token("olga"));
    let limit = std::time::Duration::from_secs(60);

    // The digest a reader's browser revalidates with: unchanged, a reader keeps it with a 304.
    let (status, headers, _) = ask_held(&state, Some(&marek), HASHED_JS, None).await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers[header::ETAG].to_str().unwrap().to_owned();
    let (status, _, body) = ask_held(&state, Some(&marek), HASHED_JS, Some(&etag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());

    // 1. A member removed from the project.
    let clock = std::time::Instant::now();
    let binding = joinedcontext_portal::resource::ResourceKey {
        namespace: joinedcontext_portal::permissions::ORG_NAMESPACE.into(),
        kind: "RoleBinding".into(),
        name: "marek-in-ovzdusie".into(),
    };
    assert!(mirror.remove(&binding).is_some());
    for (uri, etag) in [
        ("/apps/air-quality/", None),
        (HASHED_JS, Some(etag.as_str())),
    ] {
        let (status, _, _) = ask_held(&state, Some(&marek), uri, etag).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "removed member on {uri}");
    }
    assert!(clock.elapsed() < limit);

    // 2. Visibility narrowed from the organization to the project.
    set("visibility", "organization");
    assert_eq!(
        ask_held(&state, Some(&eva), HASHED_JS, None).await.0,
        StatusCode::OK
    );
    let clock = std::time::Instant::now();
    set("visibility", "project");
    for uri in ["/apps/air-quality/", HASHED_JS] {
        let (status, _, _) = ask_held(&state, Some(&eva), uri, Some(&etag)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "narrowed on {uri}");
    }
    assert!(clock.elapsed() < limit);

    // 3. A session ended (back-channel logout): the token it issued opens nothing.
    assert_eq!(
        ask_held(&state, Some(&olga), "/apps/air-quality/", None)
            .await
            .0,
        StatusCode::OK
    );
    let clock = std::time::Instant::now();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    state
        .revoke_subject("sub-olga", i64::try_from(now).unwrap() + 1)
        .await;
    for uri in ["/apps/air-quality/", HASHED_JS] {
        let (status, _, _) = ask_held(&state, Some(&olga), uri, Some(&etag)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "ended session on {uri}");
    }
    assert!(clock.elapsed() < limit);

    // 4. The App retired: a 404 for a role holder too.
    let jana = common::REALM.person_token("air-quality", "jana", &["viewer"]);
    set("visibility", "roles");
    assert_eq!(
        ask_held(&state, Some(&jana), HASHED_JS, None).await.0,
        StatusCode::OK
    );
    let clock = std::time::Instant::now();
    set("lifecycle", "retired");
    for uri in [
        "/apps/air-quality/",
        HASHED_JS,
        "/apps/air-quality/bundle.js",
    ] {
        let (status, _, _) = ask_held(&state, Some(&jana), uri, Some(&etag)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "retired on {uri}");
    }
    assert!(clock.elapsed() < limit);
}
