//! Basemap style and tile proxy routes for application map views (AP-67, T-0555).
//!
//! Sandboxed preview frames have no session and cannot reach third-party map providers
//! without leaking user activity and coordinates. All basemap requests flow through this
//! unauthenticated platform route, returning the configured style and cached raster tiles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::{Path as AxPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::config::Config;
use crate::error::ProblemDetails;
use crate::state::AppState;

/// Shared HTTP client for fetching upstream tiles.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("joinedcontext-portal basemap")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

// ponytail: the token buckets live in one replica's memory, so N replicas allow N times the cap;
// share them only when the Portal runs more than one pod. The client is the last
// X-Forwarded-For hop, the address the edge appended (see `client_ip`).
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

static RATE_LIMITER: LazyLock<Mutex<HashMap<String, TokenBucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The most buckets held at once: past it the idle ones go, and if every bucket is fresh the
/// longest-idle one makes room, so many addresses inside two minutes cannot grow the map.
const MAX_BUCKETS: usize = 10_000;

/// The most bytes one tile may hold; a longer answer is refused while it is read.
const MAX_TILE_BYTES: usize = 1024 * 1024;

fn check_rate_limit(client_ip: &str) -> bool {
    let mut map = RATE_LIMITER.lock().unwrap_or_else(|e| e.into_inner());
    admit(&mut map, client_ip, Instant::now())
}

fn admit(map: &mut HashMap<String, TokenBucket>, client_ip: &str, now: Instant) -> bool {
    if map.len() >= MAX_BUCKETS && !map.contains_key(client_ip) {
        map.retain(|_, b| now.duration_since(b.last_refill).as_secs() < 120);
        if map.len() >= MAX_BUCKETS {
            let oldest = map
                .iter()
                .min_by_key(|(_, b)| b.last_refill)
                .map(|(ip, _)| ip.clone());
            if let Some(ip) = oldest {
                map.remove(&ip);
            }
        }
    }

    let bucket = map
        .entry(client_ip.to_string())
        .or_insert_with(|| TokenBucket {
            tokens: 600.0,
            last_refill: now,
        });
    let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * 10.0).min(600.0);
    bucket.last_refill = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

/// The bucket's key: the LAST `X-Forwarded-For` entry, which APISIX appends from the peer it
/// saw, as the gateway's `address_key` reads it. A client that sends the header itself only
/// prepends to that value, so a fresh made-up first entry is never a fresh bucket (T-5566).
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.rsplit(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn problem_response(
    status: StatusCode,
    slug: &str,
    title: &str,
    detail: Option<String>,
) -> Response {
    let problem = ProblemDetails {
        r#type: format!("https://joinedcontext.com/errors/{slug}"),
        title: title.to_string(),
        status: status.as_u16(),
        detail,
        instance: None,
        hint: crate::error::hint_of(slug),
        errors: None,
    };
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ],
        serde_json::to_string(&problem).unwrap_or_default(),
    )
        .into_response()
}

/// Returns the absolute style URL for a given project if basemap proxying is configured.
pub fn style_url(config: &Config, project: &str) -> Option<String> {
    config.basemap.as_ref()?;
    Some(style_url_at(config.public_base().as_str(), project))
}

/// The style URL of `project` under the Portal's public `base`, for a caller that knows the
/// basemap is configured.
pub fn style_url_at(base: &str, project: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/api/v1/projects/{project}/basemap/default/style.json")
}

/// Everything the basemap serves for `project` lives under this prefix: the one address a
/// preview document may connect to (SDK-16).
pub fn route_prefix(config: &Config, project: &str) -> Option<String> {
    config.basemap.as_ref()?;
    Some(route_prefix_at(config.public_base().as_str(), project))
}

/// The route prefix of `project` under the Portal's public `base`, for a caller that knows the
/// basemap is configured.
pub fn route_prefix_at(base: &str, project: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/api/v1/projects/{project}/basemap/")
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/basemap/{style}/style.json",
    summary = "Read Basemap Style",
    description = "The MapLibre style of one basemap, pointing its tiles at this Portal. 404 when the installation configures no basemap.",
    tag = "basemap",
    params(
        ("project" = String, Path, description = "Project name"),
        ("style" = String, Path, description = "Basemap style (default)"),
    ),
    responses(
        (status = 200, description = "MapLibre style v8 JSON", content_type = "application/json"),
        (status = 404, description = "Not found or basemap not configured", body = ProblemDetails)
    )
)]
pub async fn get_style(
    State(state): State<AppState>,
    AxPath((project, style)): AxPath<(String, String)>,
) -> Response {
    let Some(basemap) = state.config.basemap.as_ref() else {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some("basemap is not configured on this platform".to_string()),
        );
    };

    if style != "default" {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("basemap style '{style}' not found")),
        );
    }

    if !state.mirror.namespaces().iter().any(|p| p == &project) {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("project '{project}' not found")),
        );
    }

    let base = state.config.public_base();
    let base = base.as_str().trim_end_matches('/');
    let tile_url =
        format!("{base}/api/v1/projects/{project}/basemap/default/{{z}}/{{x}}/{{y}}.png");
    let style_json = serde_json::json!({
        "version": 8,
        "sources": {
            "raster-tiles": {
                "type": "raster",
                "tiles": [tile_url],
                "tileSize": 256,
                "maxzoom": basemap.max_zoom,
                "attribution": basemap.attribution,
            }
        },
        "layers": [
            {
                "id": "raster-layer",
                "type": "raster",
                "source": "raster-tiles",
                "minzoom": 0,
                "maxzoom": basemap.max_zoom,
            }
        ]
    });

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ],
        serde_json::to_string_pretty(&style_json).unwrap_or_default(),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/basemap/{style}/{z}/{x}/{tile}",
    summary = "Read Basemap Tile",
    description = "One tile of a basemap, served from the Portal's cache or fetched from the configured tile source. Rate limited; a tile source that fails answers 502.",
    tag = "basemap",
    params(
        ("project" = String, Path, description = "Project name"),
        ("style" = String, Path, description = "Basemap style (default)"),
        ("z" = u8, Path, description = "Zoom level"),
        ("x" = u32, Path, description = "Tile X coordinate"),
        ("tile" = String, Path, description = "Tile Y coordinate with extension, e.g. 0.png"),
    ),
    responses(
        (status = 200, description = "Tile image", content_type = "image/png"),
        (status = 400, description = "Invalid tile coordinates", body = ProblemDetails),
        (status = 404, description = "Not found or basemap not configured", body = ProblemDetails),
        (status = 429, description = "Rate limit exceeded", body = ProblemDetails),
        (status = 502, description = "Bad gateway from upstream", body = ProblemDetails)
    )
)]
pub async fn get_tile(
    State(state): State<AppState>,
    AxPath((project, style, z_str, x_str, tile)): AxPath<(String, String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(basemap) = state.config.basemap.as_ref() else {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some("basemap is not configured on this platform".to_string()),
        );
    };

    if style != "default" {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("basemap style '{style}' not found")),
        );
    }

    if !state.mirror.namespaces().iter().any(|p| p == &project) {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("project '{project}' not found")),
        );
    }

    let z = match z_str.parse::<u8>() {
        Ok(val) => val,
        Err(_) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(format!("invalid zoom level: {z_str}")),
            );
        }
    };
    let x = match x_str.parse::<u32>() {
        Ok(val) => val,
        Err(_) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(format!("invalid tile x coordinate: {x_str}")),
            );
        }
    };
    let (y_str, ext) = match tile.split_once('.') {
        Some((y, ext)) => (y, ext),
        None => (tile.as_str(), "png"),
    };
    let y = match y_str.parse::<u32>() {
        Ok(val) => val,
        Err(_) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(format!("invalid tile y coordinate: {y_str}")),
            );
        }
    };

    if ext != "png" && ext != "jpg" && ext != "jpeg" {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid Request",
            Some(format!("unsupported tile extension: {ext}")),
        );
    }

    if z > basemap.max_zoom {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid Request",
            Some(format!(
                "zoom level {z} exceeds maximum zoom {}",
                basemap.max_zoom
            )),
        );
    }

    let limit = 1u64 << z;
    if (x as u64) >= limit || (y as u64) >= limit {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid Request",
            Some(format!(
                "coordinates ({x}, {y}) out of range for zoom level {z} (max {limit})"
            )),
        );
    }

    let ip = client_ip(&headers);
    if !check_rate_limit(&ip) {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too-many-requests",
            "Too Many Requests",
            Some("rate limit of 600 requests per minute exceeded".to_string()),
        );
    }

    let cache_key = format!("{style}_{z}_{x}_{y}.{ext}");
    let cache_path = basemap.cache_dir.join(&cache_key);

    if let Ok(metadata) = tokio::fs::metadata(&cache_path).await {
        let fresh = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed.as_secs() < basemap.cache_ttl_secs);
        if fresh {
            if let Ok(bytes) = tokio::fs::read(&cache_path).await {
                metrics::counter!("jc_basemap_tiles_total", "result" => "hit").increment(1);
                let content_type = if ext == "jpg" || ext == "jpeg" {
                    "image/jpeg"
                } else {
                    "image/png"
                };
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                        (
                            header::CACHE_CONTROL,
                            HeaderValue::from_static("public, max-age=3600"),
                        ),
                        (
                            header::ACCESS_CONTROL_ALLOW_ORIGIN,
                            HeaderValue::from_static("*"),
                        ),
                    ],
                    bytes,
                )
                    .into_response();
            }
        }
    }

    let mut upstream_url = basemap.url_template.clone();
    upstream_url = upstream_url
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
        .replace("{ext}", ext)
        .replace("{key}", basemap.key.as_deref().unwrap_or(""));

    let is_https = upstream_url.starts_with("https://");
    let is_allowed_http = basemap.allow_http && upstream_url.starts_with("http://");
    if !is_https && !is_allowed_http {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "upstream-unavailable",
            "Upstream Unavailable",
            Some("upstream URL scheme must be https".to_string()),
        );
    }

    let response = match HTTP_CLIENT.get(&upstream_url).send().await {
        Ok(res) => res,
        Err(e) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_GATEWAY,
                "upstream-unavailable",
                "Upstream Unavailable",
                // A reqwest error names its URL, and the URL carries the key.
                Some(format!("upstream fetch failed: {}", e.without_url())),
            );
        }
    };

    if !response.status().is_success() {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "upstream-unavailable",
            "Upstream Unavailable",
            Some(format!("upstream returned HTTP {}", response.status())),
        );
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let is_valid_image =
        content_type.starts_with("image/png") || content_type.starts_with("image/jpeg");
    if !is_valid_image {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "upstream-unavailable",
            "Upstream Unavailable",
            Some(format!(
                "upstream returned non-image content: {content_type}"
            )),
        );
    }

    let bytes = match jc_core::outbound::read_capped(response, MAX_TILE_BYTES).await {
        Ok(b) => b,
        Err(jc_core::outbound::BodyError::TooLarge(_)) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_GATEWAY,
                "upstream-unavailable",
                "Upstream Unavailable",
                Some("upstream tile exceeds 1 MiB limit".to_string()),
            );
        }
        Err(e) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_GATEWAY,
                "upstream-unavailable",
                "Upstream Unavailable",
                Some(format!("failed to read upstream body: {e}")),
            );
        }
    };

    // The cache walk reads the whole directory, so it runs off the async workers.
    let (dir, max_bytes, stored) = (
        basemap.cache_dir.clone(),
        basemap.cache_max_bytes,
        bytes.clone(),
    );
    note_cache_write(
        tokio::task::spawn_blocking(move || save_and_evict(&dir, &cache_path, &stored, max_bytes))
            .await,
    );
    metrics::counter!("jc_basemap_tiles_total", "result" => "fetch").increment(1);

    let final_ct = if content_type.starts_with("image/jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(final_ct)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// Logs a cache write that did not finish; the tile is served either way.
fn note_cache_write(done: Result<(), tokio::task::JoinError>) {
    if let Err(err) = done {
        tracing::warn!(error = %err, "basemap cache write did not finish");
    }
}

fn save_and_evict(cache_dir: &Path, file_path: &Path, bytes: &[u8], max_bytes: u64) {
    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        tracing::warn!("failed to create basemap cache dir: {e}");
        return;
    }
    if let Err(e) = std::fs::write(file_path, bytes) {
        tracing::warn!("failed to write basemap cache tile: {e}");
        return;
    }

    struct Entry {
        path: PathBuf,
        size: u64,
        modified: SystemTime,
    }

    let mut entries: Vec<Entry> = std::fs::read_dir(cache_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| Entry {
                path: entry.path(),
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect();
    let mut total_size: u64 = entries.iter().map(|entry| entry.size).sum();

    if total_size > max_bytes {
        entries.sort_by_key(|e| e.modified);
        for entry in entries {
            if total_size <= max_bytes {
                break;
            }
            if let Ok(()) = std::fs::remove_file(&entry.path) {
                total_size = total_size.saturating_sub(entry.size);
            }
        }
    }
}

/// The basemap routes for application views.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/basemap/{style}/style.json",
            get(get_style),
        )
        .route(
            "/projects/{project}/basemap/{style}/{z}/{x}/{tile}",
            get(get_tile),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_str(value).expect("header"),
        );
        headers
    }

    /// A cache folder that cannot be made, or a tile that cannot be written, leaves the tile
    /// served and the cache as it was.
    #[test]
    fn a_cache_that_cannot_be_written_is_left_alone() {
        let root = std::env::temp_dir().join(format!("jc-bm-unit-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        let file = root.join("a-file");
        std::fs::write(&file, b"x").expect("file");
        // A folder inside a file cannot be made.
        let under_file = file.join("cache");
        save_and_evict(&under_file, &under_file.join("t.png"), b"tile", 1024);
        assert!(!under_file.exists());
        // A tile path that is a folder cannot be written.
        let folder = root.join("folder.png");
        std::fs::create_dir_all(&folder).expect("folder");
        save_and_evict(&root, &folder, b"tile", 1024);
        assert!(folder.is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_forged_leftmost_forwarded_for_buys_no_new_bucket() {
        let edge_saw = "198.51.100.77";
        for _ in 0..600 {
            assert!(check_rate_limit(&client_ip(&forwarded(edge_saw))));
        }
        for forged in ["10.9.9.1", "203.0.113.5, 10.9.9.2", "garbage"] {
            let ip = client_ip(&forwarded(&format!("{forged}, {edge_saw}")));
            assert_eq!(ip, edge_saw);
            assert!(!check_rate_limit(&ip), "{forged} opened a fresh bucket");
        }
    }

    /// A full map whose buckets all went idle past two minutes drops them for a new address;
    /// a limiter a panicking thread left poisoned still admits (T-7909).
    #[test]
    fn idle_buckets_make_room_and_a_poisoned_limiter_still_admits() {
        let start = Instant::now();
        let mut map = HashMap::new();
        for n in 0..MAX_BUCKETS {
            admit(&mut map, &format!("10.1.{}.{}", n / 256, n % 256), start);
        }
        assert_eq!(map.len(), MAX_BUCKETS);
        assert!(admit(&mut map, "new", start + Duration::from_secs(121)));
        assert_eq!(map.len(), 1);

        let poisoned = std::thread::spawn(|| {
            let _held = RATE_LIMITER.lock().expect("lock");
            panic!("poison the limiter");
        })
        .join();
        assert!(poisoned.is_err());
        assert!(check_rate_limit("poisoned-limiter-test"));
    }

    /// Eviction counts files only: a folder in the cache is neither weighed nor removed.
    #[test]
    fn eviction_weighs_files_and_leaves_folders() {
        let root = std::env::temp_dir().join(format!(
            "jc-bm-evict-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        std::fs::create_dir_all(root.join("sub")).expect("sub");
        std::fs::write(root.join("old.png"), [0u8; 64]).expect("old");
        save_and_evict(&root, &root.join("new.png"), &[1u8; 64], 100);
        assert!(root.join("sub").is_dir());
        let left: Vec<_> = std::fs::read_dir(&root)
            .expect("dir")
            .flatten()
            .filter(|e| e.path().is_file())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn many_fresh_addresses_never_grow_the_bucket_map_past_its_cap() {
        let mut map = HashMap::new();
        let now = Instant::now();
        for n in 0..MAX_BUCKETS + 100 {
            assert!(admit(&mut map, &format!("fresh-{n}"), now));
        }
        assert_eq!(map.len(), MAX_BUCKETS);
    }

    #[test]
    fn a_request_without_the_edge_header_shares_the_in_cluster_bucket() {
        assert_eq!(client_ip(&HeaderMap::new()), "127.0.0.1");
        assert_eq!(client_ip(&forwarded(" ")), "127.0.0.1");
    }

    /// T-7917: an old tile the cache cannot remove (its folder is read-only) stays, the walk
    /// goes on, and the tile just fetched is still written.
    #[test]
    fn an_old_tile_that_cannot_be_removed_stays_and_the_new_one_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!(
            "jc-bm-stuck-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let cache = base.join("cache");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        std::fs::write(cache.join("old.png"), [0u8; 64]).expect("old");
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o555)).expect("lock");
        // A process that may write into a read-only folder (root) can remove the tile too.
        let privileged = std::fs::write(cache.join("probe"), b"").is_ok();
        let _ = std::fs::remove_file(cache.join("probe"));

        save_and_evict(&cache, &elsewhere.join("new.png"), &[1u8; 64], 10);

        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).expect("unlock");
        assert!(elsewhere.join("new.png").is_file(), "the new tile is kept");
        assert_eq!(
            cache.join("old.png").is_file(),
            !privileged,
            "the old tile stays exactly when it cannot be removed"
        );
        std::fs::remove_dir_all(&base).expect("cleanup");
    }

    /// T-7917: a cache write that panicked is logged and does not stop the tile being served.
    #[tokio::test]
    async fn a_cache_write_that_did_not_finish_is_only_logged() {
        let panicked = tokio::task::spawn_blocking(|| {
            None::<()>.expect("the test makes the cache write fail");
        })
        .await;
        assert!(panicked.is_err());
        note_cache_write(panicked);
        note_cache_write(Ok(()));
    }

    /// T-7917: a cache folder that cannot be listed (no read permission) evicts nothing, and the
    /// tile just written stays.
    #[test]
    fn a_cache_folder_that_cannot_be_listed_evicts_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!(
            "jc-bm-unlisted-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o300)).expect("lock");

        save_and_evict(&cache, &cache.join("new.png"), &[1u8; 64], 10);

        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).expect("unlock");
        assert!(cache.join("new.png").is_file(), "the tile is written");
        std::fs::remove_dir_all(&base).expect("cleanup");
    }

    /// T-7917: when every bucket is fresh and the map is full, the longest-idle one makes room
    /// for a new address.
    #[test]
    fn a_full_map_of_fresh_buckets_drops_its_longest_idle_one() {
        let mut map = HashMap::new();
        let start = Instant::now();
        for n in 0..MAX_BUCKETS {
            assert!(admit(&mut map, &format!("ip-{n}"), start));
        }
        assert!(admit(&mut map, "newcomer", start));
        assert_eq!(map.len(), MAX_BUCKETS);
        assert!(map.contains_key("newcomer"));
    }
}
