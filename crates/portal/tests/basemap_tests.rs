//! Integration tests for the platform basemap tile and style routes (AP-67, T-0555).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::IntoResponse;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use joinedcontext_portal::config::{BasemapConfig, Config, ConfigError};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::Value;
use tower::ServiceExt;

#[path = "common/unique.rs"]
mod unique;

const PROJECT: &str = "helsinki";

fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ContextSpace".into(),
        metadata: ObjectMeta {
            name: "helsinki".into(),
            namespace: Some(PROJECT.into()),
            ..Default::default()
        },
        spec: serde_json::json!({}),
        status: None,
    });
    mirror
}

async fn start_upstream() -> (String, Arc<AtomicUsize>) {
    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = call_count.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = axum::Router::new().route(
        "/{z}/{x}/{tile}",
        axum::routing::get(
            move |axum::extract::Path((_z, _x, tile)): axum::extract::Path<(u8, u32, String)>| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if tile.starts_with("98.") {
                        // Declares 50 MiB, sends one chunk and then nothing more.
                        let first = futures_util::stream::once(async {
                            Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![0u8; 1024]))
                        });
                        let body = Body::from_stream(first.chain(futures_util::stream::pending()));
                        return (
                            [
                                (header::CONTENT_TYPE, "image/png".to_owned()),
                                (header::CONTENT_LENGTH, (50 * 1024 * 1024).to_string()),
                            ],
                            body,
                        )
                            .into_response();
                    }
                    if tile.starts_with("97.") {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "down").into_response();
                    }
                    if tile.starts_with("96.") {
                        return (
                            [(header::CONTENT_TYPE, "image/jpeg")],
                            vec![0xff, 0xd8, 0xff, 1],
                        )
                            .into_response();
                    }
                    if tile.starts_with("95.") {
                        // Sends one chunk, then breaks the body off.
                        let first = futures_util::stream::once(async {
                            Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![0u8; 16]))
                        });
                        let cut = futures_util::stream::once(async {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            Err(std::io::Error::other("cut"))
                        });
                        let broken = first.chain(cut);
                        return (
                            [(header::CONTENT_TYPE, "image/png")],
                            Body::from_stream(broken),
                        )
                            .into_response();
                    }
                    if tile.starts_with("non_image") || tile.starts_with("99.") {
                        (
                            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                            "<html><body>not an image</body></html>",
                        )
                            .into_response()
                    } else {
                        (
                            [(header::CONTENT_TYPE, "image/png")],
                            vec![
                                0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 1, 2, 3, 4,
                            ],
                        )
                            .into_response()
                    }
                }
            },
        ),
    );

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (addr.to_string(), call_count)
}

async fn call_app(app: &axum::Router, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

#[tokio::test]
async fn not_configured_answers_404_problem_on_both_routes() {
    let config = Config::for_tests();
    let state = AppState::new(config, None).with_mirror(mirror());
    let app = server::app(state);

    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/style.json"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 404);

    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/0/0/0.png"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 404);
}

#[tokio::test]
async fn style_json_carries_attribution_and_tile_url() {
    let (addr, _) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-style-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    config.basemap = Some(BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png?key={{key}}"),
        "© OpenStreetMap contributors".to_string(),
        temp_dir.clone(),
    ));

    let state = AppState::new(config, None).with_mirror(mirror());
    let app = server::app(state);

    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/style.json"),
    )
    .await;
    let _ = std::fs::remove_dir_all(&temp_dir);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["version"], 8);
    assert_eq!(
        json["sources"]["raster-tiles"]["attribution"],
        "© OpenStreetMap contributors"
    );
    let tiles = json["sources"]["raster-tiles"]["tiles"].as_array().unwrap();
    assert!(tiles[0].as_str().unwrap().ends_with(&format!(
        "/api/v1/projects/{PROJECT}/basemap/default/{{z}}/{{x}}/{{y}}.png"
    )));
}

#[tokio::test]
async fn configured_tile_served_and_cached_and_key_redacted() {
    let (addr, calls) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-cache-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    let secret_key = "super-secret-basemap-key-98765";
    let mut bm = BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png?key={{key}}"),
        "OSM".to_string(),
        temp_dir.clone(),
    );
    bm.key = Some(secret_key.to_string());
    config.basemap = Some(bm);

    let state = AppState::new(config, None).with_mirror(mirror());
    let app = server::app(state);

    // Verify key does not appear in style.json
    let (_, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/style.json"),
    )
    .await;
    assert!(!String::from_utf8_lossy(&body).contains(secret_key));
    for (name, val) in &headers {
        assert!(!name.as_str().contains(secret_key));
        assert!(!val.to_str().unwrap_or("").contains(secret_key));
    }

    // First tile fetch: hits upstream
    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/0/0/0.png"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "image/png");
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    assert_eq!(
        body,
        vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 1, 2, 3, 4]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!String::from_utf8_lossy(&body).contains(secret_key));

    // Second tile fetch: served from disk cache, no upstream hit
    let (status2, _, body2) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/0/0/0.png"),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body2, body);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// An unknown style is 404; a tile without an extension is a png; a jpeg keeps its type from the
/// upstream and from the cache; an upstream error or a body broken off is 502; one address past
/// 600 tiles a minute is 429.
#[tokio::test]
async fn styles_types_upstream_failures_and_the_rate_limit() {
    let (addr, calls) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-edge-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    config.basemap = Some(BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"),
        "OSM".to_string(),
        temp_dir.clone(),
    ));
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));
    let tile = |rest: &str| format!("/api/v1/projects/{PROJECT}/basemap/default/{rest}");

    let (status, _, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/satellite/0/0/0.png"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("'satellite'"));

    let (status, headers, _) = call_app(&app, &tile("7/0/1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).expect("type"),
        "image/png"
    );

    for _ in 0..2 {
        let (status, headers, body) = call_app(&app, &tile("7/0/96.jpg")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get(header::CONTENT_TYPE).expect("type"),
            "image/jpeg"
        );
        assert_eq!(body, [0xff, 0xd8, 0xff, 1]);
    }

    for (rest, says) in [("7/0/97.png", "HTTP 500"), ("7/0/95.png", "upstream body")] {
        let (status, _, body) = call_app(&app, &tile(rest)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{rest}");
        assert!(
            String::from_utf8_lossy(&body).contains(says),
            "{rest}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let before = calls.load(Ordering::SeqCst);

    let ip = "198.51.100.78";
    let mut refused = None;
    // The bucket refills ten a second, so a few more than 600 run before the refusal.
    for _ in 0..3000 {
        let req = Request::builder()
            .method(Method::GET)
            .uri(tile("7/0/96.jpg"))
            .header("x-forwarded-for", ip)
            .body(Body::empty())
            .expect("request");
        let resp = app.clone().oneshot(req).await.expect("response");
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(resp.status());
            break;
        }
    }
    assert_eq!(refused, Some(StatusCode::TOO_MANY_REQUESTS));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "the cache answered every tile"
    );
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn an_unreachable_upstream_answers_502_without_the_key() {
    // A port that was just bound and released: nothing listens there.
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let secret_key = "super-secret-basemap-key-unreachable";
    let mut config = Config::for_tests();
    let mut bm = BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png?key={{key}}"),
        "OSM".to_string(),
        std::env::temp_dir().join("jc-bm-unreachable"),
    );
    bm.key = Some(secret_key.to_string());
    config.basemap = Some(bm);
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));

    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/1/1/1.png"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(!String::from_utf8_lossy(&body).contains(secret_key));
    for value in headers.values() {
        assert!(!value.to_str().unwrap_or("").contains(secret_key));
    }
}

/// A plain-http upstream the configuration does not allow is refused before any call, and an
/// upstream that is neither https nor http is refused too (T-7909).
#[tokio::test]
async fn an_upstream_scheme_the_configuration_does_not_allow_is_refused_before_any_call() {
    let (addr, calls) = start_upstream().await;
    for (template, allow_http) in [
        (format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"), false),
        (format!("ftp://{addr}/{{z}}/{{x}}/{{y}}.png"), true),
    ] {
        let mut config = Config::for_tests();
        let mut bm = BasemapConfig::for_tests(
            template.clone(),
            "OSM".to_string(),
            std::env::temp_dir().join(format!("jc-bm-scheme-{}", crate::unique::unique())),
        );
        bm.allow_http = allow_http;
        config.basemap = Some(bm);
        let app = server::app(AppState::new(config, None).with_mirror(mirror()));
        let (status, _, body) = call_app(
            &app,
            &format!("/api/v1/projects/{PROJECT}/basemap/default/2/1/1.png"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{template}");
        assert!(
            String::from_utf8_lossy(&body).contains("https"),
            "{template}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn out_of_range_coordinates_refused_before_upstream_call() {
    let (addr, calls) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-range-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    config.basemap = Some(BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"),
        "OSM".to_string(),
        temp_dir.clone(),
    ));

    let state = AppState::new(config, None).with_mirror(mirror());
    let app = server::app(state);

    // At z=0, x must be < 2^0 = 1, so x=1 is out of range
    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/0/1/0.png"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 400);

    // Zoom level 25 exceeds max zoom 19
    let (status, _, _) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/25/0/0.png"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Paths that try to leave the template: an encoded separator, a traversal, a foreign type.
    for tile in [
        "0/..%2F..%2Fsecret.png",
        "..%2F0.png",
        "0.svg",
        "0.png%3Fkey=x",
        "-1.png",
    ] {
        let (status, _, _) = call_app(
            &app,
            &format!("/api/v1/projects/{PROJECT}/basemap/default/1/0/{tile}"),
        )
        .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{tile} answered {status}"
        );
    }

    // Zero upstream calls were made
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn cache_evicts_past_its_cap() {
    let (addr, _) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-evict-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    let mut bm = BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"),
        "OSM".to_string(),
        temp_dir.clone(),
    );
    // Cap is 15 bytes. A tile is 12 bytes. Two tiles = 24 bytes > 15 bytes.
    bm.cache_max_bytes = 15;
    config.basemap = Some(bm);

    let state = AppState::new(config, None).with_mirror(mirror());
    let app = server::app(state);

    // Request tile 1/0/0
    let (status1, _, _) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/1/0/0.png"),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Request tile 1/1/1
    let (status2, _, _) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/1/1/1.png"),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);

    let files: Vec<_> = std::fs::read_dir(&temp_dir)
        .unwrap()
        .map(|r| r.unwrap().path())
        .collect();
    assert_eq!(
        files.len(),
        1,
        "older tile should have been evicted; files: {files:?}"
    );
    assert!(files[0].to_string_lossy().contains("1_1_1"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// A tile that declares more than the cap is refused at its headers, not after the upstream's
/// whole body (or the 10 s client timeout) has passed.
#[tokio::test]
async fn an_oversized_tile_is_refused_before_its_body_is_read() {
    let (addr, _) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-big-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    config.basemap = Some(BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"),
        "OSM".to_string(),
        temp_dir.clone(),
    ));
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));

    let answer = tokio::time::timeout(
        Duration::from_secs(3),
        call_app(
            &app,
            &format!("/api/v1/projects/{PROJECT}/basemap/default/7/0/98.png"),
        ),
    )
    .await;
    let (status, _, body) = answer.expect("refused at the declared length, within 3 s");
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["detail"], "upstream tile exceeds 1 MiB limit");
    assert!(!temp_dir.join("7/0/98.png").exists());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn non_image_upstream_answer_refused_with_502() {
    let (addr, calls) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("jc-bm-502-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    config.basemap = Some(BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"),
        "OSM".to_string(),
        temp_dir.clone(),
    ));

    let state = AppState::new(config, None).with_mirror(mirror());
    let app = server::app(state);

    // Coordinate y=99 triggers HTML response in the stub upstream
    let (status, headers, body) = call_app(
        &app,
        &format!("/api/v1/projects/{PROJECT}/basemap/default/7/0/99.png"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 502);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn http_upstream_rejected_from_environment() {
    let err = Config::from_vars(|k| match k {
        "JC_BASEMAP_URL" => Some("http://127.0.0.1:8080/{z}/{x}/{y}.png".to_string()),
        "JC_BASEMAP_ATTRIBUTION" => Some("Attribution".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(matches!(
        err,
        ConfigError::Invalid {
            var: "JC_BASEMAP_URL",
            ..
        }
    ));
}

/// A configured basemap on a fresh cache folder, upstream tiles counted.
async fn cached_app(
    ttl: u64,
    prefix: &str,
) -> (axum::Router, Arc<AtomicUsize>, std::path::PathBuf) {
    let (addr, calls) = start_upstream().await;
    let temp_dir = std::env::temp_dir().join(format!("{prefix}-{}", crate::unique::unique()));
    let mut config = Config::for_tests();
    let mut bm = BasemapConfig::for_tests(
        format!("http://{addr}/{{z}}/{{x}}/{{y}}.png"),
        "OSM".to_string(),
        temp_dir.clone(),
    );
    bm.cache_ttl_secs = ttl;
    config.basemap = Some(bm);
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));
    (app, calls, temp_dir)
}

const TILE: &str = "/api/v1/projects/helsinki/basemap/default/1/0/0.png";

/// T-7917: a cached tile older than the configured lifetime is fetched again, not served.
#[tokio::test]
async fn a_cached_tile_past_its_lifetime_is_fetched_again() {
    let (app, calls, dir) = cached_app(0, "jc-bm-stale").await;
    for fetched in [1, 2] {
        let (status, _, body) = call_app(&app, TILE).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 12);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            fetched,
            "a stale tile is no hit"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-7917: a cached tile stamped in the future (a clock that moved back) cannot be aged, so it is
/// not trusted: the upstream is asked again and the tile is replaced.
#[tokio::test]
async fn a_cached_tile_from_the_future_is_fetched_again() {
    let (app, calls, dir) = cached_app(3600, "jc-bm-future").await;
    let (status, _, _) = call_app(&app, TILE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let cached = dir.join("default_1_0_0.png");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&cached)
        .expect("the cached tile")
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
        .expect("a later stamp");
    let (status, _, body) = call_app(&app, TILE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 12);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the future tile was served"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-7917: a cache entry that cannot be read (here a folder where the tile should be) is no
/// reason to fail the tile: it comes from the upstream and the entry is left as it was.
#[tokio::test]
async fn an_unreadable_cache_entry_still_serves_the_tile_from_upstream() {
    let (app, calls, dir) = cached_app(3600, "jc-bm-unreadable").await;
    let entry = dir.join("default_1_0_0.png");
    std::fs::create_dir_all(&entry).expect("a folder in the tile's place");
    let (status, _, body) = call_app(&app, TILE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 12);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(entry.is_dir(), "the entry stays what it was");
    let _ = std::fs::remove_dir_all(&dir);
}
