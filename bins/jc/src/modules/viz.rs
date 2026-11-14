//! viz (the chart renderer and live-data explorer of joinedcontext-deployment `components/viz`)
//! inside `jc`: the files of `vendor/viz` (vendor/SOURCES.md, `vendor/viz/SHA256SUMS`) with the
//! headers its `nginx.conf` sends. The catalogue's dataset page frames `/explore.html` for the
//! "Živé dáta" grid and map (G1); `/embed.html` is the same page for any site, `/` the chart host
//! and `/render.html` its sandboxed renderer.
//!
//! The deployment's pages find the gateway, tiles and catalogue by their own host
//! (`viz.{domain}` → `api.`, `tiles.`, `opendata.{domain}`). Here they answer beside other modules,
//! so `/explore.html` and `/embed.html` carry `<meta name="jc-viz">` naming those three addresses
//! (never a URL parameter), and each page's CSP names exactly them.
//!
//! The pages render untrusted Endpoint data. On dev and prod they live on `viz.{domain}`, an
//! origin of their own. When this mount shares its origin with the Portal, the catalogue or the
//! gateway (slug mode: one host for all), every page also gets a CSP `sandbox` without
//! `allow-same-origin`, so it runs in an opaque origin and reads no cookie and makes no
//! credentialed call there, framed or opened on its own. Its reads then go out as `Origin: null`:
//! the gateway answers those anonymously (`modules::gateway`) and `vendor/` answers CORS for the
//! MapLibre module `import()`.

use crate::siblings::LiveAddresses;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jc_router::Mount;
use std::io::Write;
use std::sync::{Arc, LazyLock};

/// Every served file: (path under the mount, bytes). `.mjs` modules are stored gzipped.
const FILES: &[(&str, &[u8])] = &[
    (
        "csv.js",
        include_bytes!("../../../../vendor/viz/site/csv.js"),
    ),
    (
        "embed.js",
        include_bytes!("../../../../vendor/viz/site/embed.js"),
    ),
    (
        "explore.css",
        include_bytes!("../../../../vendor/viz/site/explore.css"),
    ),
    (
        "explore.js",
        include_bytes!("../../../../vendor/viz/site/explore.js"),
    ),
    (
        "host.css",
        include_bytes!("../../../../vendor/viz/site/host.css"),
    ),
    (
        "host.js",
        include_bytes!("../../../../vendor/viz/site/host.js"),
    ),
    (
        "map.css",
        include_bytes!("../../../../vendor/viz/site/map.css"),
    ),
    (
        "map.js",
        include_bytes!("../../../../vendor/viz/site/map.js"),
    ),
    (
        "refuse.js",
        include_bytes!("../../../../vendor/viz/site/refuse.js"),
    ),
    (
        "render.css",
        include_bytes!("../../../../vendor/viz/site/render.css"),
    ),
    (
        "render.js",
        include_bytes!("../../../../vendor/viz/site/render.js"),
    ),
    (
        "index.html",
        include_bytes!("../../../../vendor/viz/site/index.html"),
    ),
    (
        "explore.html",
        include_bytes!("../../../../vendor/viz/site/explore.html"),
    ),
    (
        "map.html",
        include_bytes!("../../../../vendor/viz/site/map.html"),
    ),
    (
        "render.html",
        include_bytes!("../../../../vendor/viz/site/render.html"),
    ),
    (
        "explorer/explorer.css",
        include_bytes!("../../../../vendor/viz/explorer/explorer.css"),
    ),
    (
        "explorer/explorer.js",
        include_bytes!("../../../../vendor/viz/explorer/explorer.js"),
    ),
    (
        "vendor/maplibre-gl-6.14.0.css",
        include_bytes!("../../../../vendor/viz/vendor/maplibre-gl-6.14.0.css"),
    ),
    (
        "vendor/maplibre-gl-6.14.0.mjs.gz",
        include_bytes!("../../../../vendor/viz/vendor/maplibre-gl-6.14.0.mjs.gz"),
    ),
    (
        "vendor/maplibre-gl-worker-6.14.0.mjs.gz",
        include_bytes!("../../../../vendor/viz/vendor/maplibre-gl-worker-6.14.0.mjs.gz"),
    ),
    (
        "vendor/pmtiles-4.5.0.js",
        include_bytes!("../../../../vendor/viz/vendor/pmtiles-4.5.0.js"),
    ),
    (
        "vendor/vega-6.4.0.min.js",
        include_bytes!("../../../../vendor/viz/vendor/vega-6.4.0.min.js"),
    ),
    (
        "vendor/vega-embed-7.3.0.min.js",
        include_bytes!("../../../../vendor/viz/vendor/vega-embed-7.3.0.min.js"),
    ),
    (
        "vendor/vega-interpreter-2.3.2.js",
        include_bytes!("../../../../vendor/viz/vendor/vega-interpreter-2.3.2.js"),
    ),
    (
        "vendor/vega-lite-6.5.0.min.js",
        include_bytes!("../../../../vendor/viz/vendor/vega-lite-6.5.0.min.js"),
    ),
];

const PERMISSIONS: &str = "camera=(), microphone=(), geolocation=(), payment=(), usb=()";
/// Everything that is not one of the pages: nothing it could run or frame.
const INERT: &str = "default-src 'none'; frame-ancestors 'none'";
/// A page on an origin it shares with the Portal: an opaque origin, never `allow-same-origin`.
const SANDBOX: &str =
    "sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads";

/// The addresses a page reads, and the CSP of each page, fixed at start.
struct Site {
    meta: String,
    index: String,
    render: String,
    map: String,
    explore: String,
    embed: String,
}

fn join(sources: &[&str]) -> String {
    let named: Vec<&str> = sources.iter().copied().filter(|s| !s.is_empty()).collect();
    if named.is_empty() {
        "'none'".to_owned()
    } else {
        named.join(" ")
    }
}

impl Site {
    /// `base` is this mount's public base; the addresses are bases ("" for one jc does not know).
    fn new(base: &str, live: &LiveAddresses) -> Self {
        let origin = |base: &str| crate::siblings::origin(base);
        let own = &origin(base);
        let (api, tiles, cat) = (
            origin(&live.api),
            origin(&live.tiles),
            origin(&live.catalogue),
        );
        let shared = [&api, &cat, &origin(&live.portal)]
            .iter()
            .any(|o| o.as_str() == own);
        let tail = if shared {
            format!("base-uri 'none'; form-action 'none'; {SANDBOX}")
        } else {
            "base-uri 'none'; form-action 'none'".to_owned()
        };
        // Sandboxed, the explorer reads MapLibre's worker as text to start it from a blob.
        let releases = if shared {
            format!("{base}/vendor/")
        } else {
            String::new()
        };
        let page = |img: &str, connect: &str, ancestors: &str| {
            format!(
                "default-src 'none'; script-src {own}; style-src {own} 'unsafe-inline'; img-src {own}{img} data: blob:; connect-src {connect}; worker-src blob: {own}; {tail}; frame-ancestors {ancestors}"
            )
        };
        let tiles_img = if tiles.is_empty() {
            String::new()
        } else {
            format!(" {tiles}")
        };
        let meta = serde_json::json!({
            "api": live.api, "tiles": live.tiles, "catalogue": live.catalogue,
        })
        .to_string()
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
        Self {
            meta: format!("<meta name=\"jc-viz\" content=\"{meta}\">\n"),
            index: format!(
                "default-src 'none'; script-src {own}; style-src {own}; img-src {own} data:; connect-src {}; frame-src {own}; {tail}; frame-ancestors *",
                join(&[&api, &cat])
            ),
            // The chart host frames the renderer. Sandboxed, the host is an opaque origin, which no
            // `frame-ancestors` source matches (`*` included): the renderer then names none.
            render: format!(
                "default-src 'none'; script-src {own}; style-src {own} 'unsafe-inline'; img-src {own} data:; connect-src 'none'; {tail}{}",
                if shared { "" } else { "; frame-ancestors *" }
            ),
            map: page(&tiles_img, &join(&[&tiles]), "*"),
            explore: page(
                &tiles_img,
                &join(&[&api, &tiles, &releases]),
                if cat.is_empty() { "'none'" } else { &cat },
            ),
            embed: page(&tiles_img, &join(&[&api, &tiles, &cat, &releases]), "*"),
        }
    }
}

fn file(path: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| *b)
}

/// A static file as served: its strong ETag (the `SHA256SUMS` sum of its bytes) and, when gzip
/// makes it smaller, its gzipped bytes. Pages are left out: they carry the per-site meta.
struct Packed {
    etag: String,
    gzip: Option<Vec<u8>>,
}

/// Built once (`build` forces it at start): the 1.3 MB explorer bundle goes out as about 360 kB.
/// Only public, fixed files are compressed, never a per-user body, so gzip reveals nothing.
static PACKED: LazyLock<Vec<Option<Packed>>> = LazyLock::new(|| {
    let sums = include_str!("../../../../vendor/viz/SHA256SUMS");
    FILES
        .iter()
        .map(|(path, bytes)| {
            if path.ends_with(".html") || path.ends_with(".gz") {
                return None;
            }
            let sum = sums
                .lines()
                .filter_map(|l| l.split_once("  "))
                .find(|(_, p)| p == path)?
                .0;
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            let gzip = gz
                .write_all(bytes)
                .and_then(|()| gz.finish())
                .ok()
                .filter(|z| z.len() < bytes.len());
            Some(Packed {
                etag: format!("\"{sum}\""),
                gzip,
            })
        })
        .collect()
});

fn packed(path: &str) -> Option<&'static Packed> {
    let i = FILES.iter().position(|(p, _)| *p == path)?;
    PACKED.get(i)?.as_ref()
}

/// Whether `Accept-Encoding` takes gzip (by name or `*`) with a non-zero weight.
fn takes_gzip(accept: &str) -> bool {
    accept.split(',').any(|item| {
        let mut parts = item.split(';').map(str::trim);
        let name = parts.next().unwrap_or_default();
        let refused = parts.any(|p| {
            p.strip_prefix("q=")
                .and_then(|q| q.parse::<f32>().ok())
                .is_some_and(|q| q <= 0.0)
        });
        (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
    })
}

/// Whether `If-None-Match` names `etag` (weak comparison, RFC 9110 13.1.2) or is `*`.
fn matches(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(str::trim)
        .any(|t| t == "*" || t.trim_start_matches("W/") == etag)
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn answer(status: StatusCode, ctype: &str, cache: &str, csp: &str, body: Body) -> Response {
    let mut res = (status, body).into_response();
    let h = res.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, k: header::HeaderName, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(k, v);
        }
    };
    set(h, header::CONTENT_TYPE, ctype);
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::REFERRER_POLICY, "no-referrer");
    set(
        h,
        header::HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    set(
        h,
        header::HeaderName::from_static("permissions-policy"),
        PERMISSIONS,
    );
    set(h, header::CACHE_CONTROL, cache);
    set(h, header::CONTENT_SECURITY_POLICY, csp);
    res
}

/// A public release under `vendor/` for any reader: a sandboxed page `import()`s MapLibre's module
/// and starts its worker as `Origin: null`, which needs CORS even on its own host.
fn public(res: &mut Response) {
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
}

fn serve(site: &Site, method: &Method, path: &str, req: &header::HeaderMap) -> Response {
    let not_found = || {
        answer(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "no-cache",
            INERT,
            Body::from("not found"),
        )
    };
    if method != Method::GET && method != Method::HEAD {
        let mut res = answer(
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain; charset=utf-8",
            "no-cache",
            INERT,
            Body::empty(),
        );
        res.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return res;
    }
    let path = path.trim_start_matches('/');
    let html = |name: &str, csp: &str, meta: bool| {
        let page = String::from_utf8_lossy(file(name).unwrap_or_default());
        let page = if meta {
            page.replacen("<head>\n", &format!("<head>\n{}", site.meta), 1)
        } else {
            page.into_owned()
        };
        answer(
            StatusCode::OK,
            "text/html; charset=utf-8",
            "no-cache",
            csp,
            Body::from(page),
        )
    };
    match path {
        "healthz" => answer(
            StatusCode::OK,
            "text/plain",
            "no-cache",
            INERT,
            Body::from("ok\n"),
        ),
        "" => html("index.html", &site.index, false),
        "render.html" => html("render.html", &site.render, false),
        "map.html" => html("map.html", &site.map, false),
        "explore.html" => html("explore.html", &site.explore, true),
        "embed.html" => html("explore.html", &site.embed, true),
        p if p.ends_with(".html") => not_found(),
        p => {
            // MapLibre's modules exist only gzipped; every browser takes gzip.
            // ponytail: no gunzip for a client without gzip, as nginx's `gunzip on` had.
            if let Some(b) = p
                .ends_with(".mjs")
                .then(|| file(&format!("{p}.gz")))
                .flatten()
            {
                let mut res = answer(
                    StatusCode::OK,
                    content_type(p),
                    "public, max-age=31536000, immutable",
                    INERT,
                    Body::from(b),
                );
                res.headers_mut()
                    .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
                public(&mut res);
                return res;
            }
            match file(p) {
                Some(b) if !p.ends_with(".gz") => {
                    let vendor = p.starts_with("vendor/");
                    let cache = if vendor {
                        "public, max-age=31536000, immutable"
                    } else {
                        "no-cache"
                    };
                    let value = |name: header::HeaderName| {
                        req.get(name)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default()
                    };
                    let pack = packed(p);
                    let gzip = pack
                        .and_then(|k| k.gzip.as_deref())
                        .filter(|_| takes_gzip(value(header::ACCEPT_ENCODING)));
                    // Each encoding is its own representation, so its own strong ETag.
                    let etag = pack.map(|k| {
                        if gzip.is_some() {
                            format!("{}-gzip\"", k.etag.trim_end_matches('"'))
                        } else {
                            k.etag.clone()
                        }
                    });
                    let fresh = etag
                        .as_deref()
                        .is_some_and(|e| matches(value(header::IF_NONE_MATCH), e));
                    let (status, body) = match (fresh, gzip) {
                        (true, _) => (StatusCode::NOT_MODIFIED, Body::empty()),
                        (false, Some(z)) => (StatusCode::OK, Body::from(z)),
                        (false, None) => (StatusCode::OK, Body::from(b)),
                    };
                    let mut res = answer(status, content_type(p), cache, INERT, body);
                    let h = res.headers_mut();
                    if let Some(etag) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
                        h.insert(header::ETAG, etag);
                        h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
                    }
                    if gzip.is_some() {
                        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
                    }
                    if vendor {
                        public(&mut res);
                    }
                    res
                }
                _ => not_found(),
            }
        }
    }
}

/// viz's router at `mount`, reading the gateway, tiles and catalogue at `live`.
pub fn build(live: &LiveAddresses, mount: &Mount) -> Router {
    let site = Arc::new(Site::new(&mount.public_base(), live));
    LazyLock::force(&PACKED);
    Router::new().fallback(
        move |method: Method, uri: axum::http::Uri, headers: header::HeaderMap| {
            let site = site.clone();
            async move { serve(&site, &method, uri.path(), &headers) }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn live() -> LiveAddresses {
        LiveAddresses {
            api: "https://api.city.test".into(),
            tiles: "https://tiles.city.test".into(),
            catalogue: "http://city.test:8080/opendata".into(),
            ..LiveAddresses::default()
        }
    }

    fn get(path: &str) -> Response {
        get_with(path, &[])
    }

    fn get_with(path: &str, headers: &[(header::HeaderName, &str)]) -> Response {
        let mut req = header::HeaderMap::new();
        for (k, v) in headers {
            req.insert(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        serve(
            &Site::new("http://city.test:8080", &live()),
            &Method::GET,
            path,
            &req,
        )
    }

    fn header<'a>(res: &'a Response, name: &str) -> &'a str {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }

    async fn text(res: Response) -> String {
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// Every served file is the one `vendor/viz/SHA256SUMS` names, and every listed file is served.
    #[test]
    fn every_file_matches_its_sum() {
        let sums = include_str!("../../../../vendor/viz/SHA256SUMS");
        let listed: Vec<(&str, &str)> = sums
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.split_once("  ").expect("sum  path"))
            .collect();
        assert_eq!(listed.len(), FILES.len());
        for (path, bytes) in FILES {
            let want = listed
                .iter()
                .find(|(_, p)| p == path)
                .unwrap_or_else(|| panic!("{path} not in SHA256SUMS"))
                .0;
            let got: String = Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(got, want, "{path}");
        }
    }

    /// The releases are the deployment's byte for byte and the explorer bundle is the one built
    /// from crates/portal/ui: the sums beside each hold.
    #[test]
    fn the_deployment_sums_of_vendor_and_explorer_hold() {
        for (dir, sums) in [
            (
                "vendor",
                include_str!("../../../../vendor/viz/vendor/SHA256SUMS"),
            ),
            (
                "explorer",
                include_str!("../../../../vendor/viz/explorer/SHA256SUMS"),
            ),
        ] {
            let mut n = 0;
            for (sum, name) in sums
                .lines()
                .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                .map(|l| l.split_once("  ").expect("sum  name"))
            {
                let bytes = file(&format!("{dir}/{name}"))
                    .unwrap_or_else(|| panic!("{dir}/{name} not served"));
                let got: String = Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                assert_eq!(got, sum, "{dir}/{name}");
                n += 1;
            }
            assert!(n >= 2, "{dir}");
        }
    }

    #[tokio::test]
    async fn the_explorer_names_its_hosts_in_a_meta_and_its_csp() {
        let res = get("/explore.html");
        assert_eq!(res.status(), StatusCode::OK);
        let csp = header(&res, "content-security-policy").to_owned();
        assert!(
            csp.contains(
                "connect-src https://api.city.test https://tiles.city.test http://city.test:8080/vendor/;"
            ),
            "{csp}"
        );
        assert!(
            csp.ends_with("frame-ancestors http://city.test:8080"),
            "{csp}"
        );
        assert!(csp.contains("script-src http://city.test:8080;"), "{csp}");
        assert!(!csp.contains("unsafe-eval") && !csp.contains('*'), "{csp}");
        let body = text(res).await;
        assert!(body.contains(r#"<meta name="jc-viz" content="{&quot;api&quot;:&quot;https://api.city.test&quot;,&quot;tiles&quot;:&quot;https://tiles.city.test&quot;,&quot;catalogue&quot;:&quot;http://city.test:8080/opendata&quot;}">"#), "{body}");

        let embed = get("/embed.html");
        let csp = header(&embed, "content-security-policy").to_owned();
        assert!(
            csp.contains(
                "connect-src https://api.city.test https://tiles.city.test http://city.test:8080 http://city.test:8080/vendor/;"
            ),
            "{csp}"
        );
        assert!(csp.ends_with("frame-ancestors *"), "{csp}");
        assert!(text(embed).await.contains("jc-viz"));
    }

    /// Beside the Portal or the catalogue (one origin, slug mode) every page is a CSP sandbox
    /// without `allow-same-origin`, so its scripts run in an opaque origin, framed or not; on an
    /// origin of its own (dev's viz.{domain}) none is.
    #[test]
    fn a_page_on_an_origin_it_shares_is_sandboxed_to_an_opaque_origin() {
        let sandbox =
            "sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads;";
        for shared in [
            LiveAddresses {
                catalogue: "http://localhost:42102/opendata".into(),
                ..LiveAddresses::default()
            },
            LiveAddresses {
                portal: "http://localhost:42102/portal".into(),
                ..LiveAddresses::default()
            },
            LiveAddresses {
                api: "http://localhost:42102".into(),
                ..LiveAddresses::default()
            },
        ] {
            let site = Site::new("http://localhost:42102", &shared);
            for path in [
                "/explore.html",
                "/embed.html",
                "/",
                "/map.html",
                "/render.html",
            ] {
                let res = serve(&site, &Method::GET, path, &header::HeaderMap::new());
                let csp = header(&res, "content-security-policy");
                // The renderer ends on its sandbox: it names no `frame-ancestors` (below).
                let held = if path == "/render.html" {
                    csp.ends_with(sandbox.trim_end_matches(';'))
                } else {
                    csp.contains(sandbox)
                };
                assert!(held, "{path}: {csp}");
                if path.ends_with("e.html") {
                    assert!(
                        csp.contains(" http://localhost:42102/vendor/;"),
                        "{path}: {csp}"
                    );
                }
                assert!(!csp.contains("allow-same-origin"), "{path}: {csp}");
            }
        }
        let own = Site::new("https://viz.city.test", &live());
        for path in ["/explore.html", "/embed.html", "/", "/map.html"] {
            let res = serve(&own, &Method::GET, path, &header::HeaderMap::new());
            let csp = header(&res, "content-security-policy");
            assert!(!csp.contains("sandbox"), "{path}: {csp}");
        }
    }

    /// Sandboxed, the chart host `/` is an opaque origin and frames `/render.html`: a
    /// `frame-ancestors` there, `*` included, matches no opaque ancestor and the browser blocks
    /// every chart of a dataset page (rehearsal 2026-11-09, helsinki-weather). On an origin of its
    /// own the renderer keeps `frame-ancestors *`.
    #[test]
    fn a_sandboxed_chart_host_may_frame_its_renderer() {
        let shared = LiveAddresses {
            catalogue: "http://localhost:42102/opendata".into(),
            ..LiveAddresses::default()
        };
        let site = Site::new("http://localhost:42102", &shared);
        let res = serve(
            &site,
            &Method::GET,
            "/render.html",
            &header::HeaderMap::new(),
        );
        // The whole header, pinned: only `frame-ancestors *` left it; it still runs nothing but
        // its own scripts, connects nowhere and stays sandboxed without `allow-same-origin`.
        assert_eq!(
            header(&res, "content-security-policy"),
            "default-src 'none'; script-src http://localhost:42102; style-src http://localhost:42102 'unsafe-inline'; img-src http://localhost:42102 data:; connect-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads"
        );
        assert!(res.headers().get("x-frame-options").is_none());
        // Every other page answers the framing rule it answered before.
        let ancestors = |path: &str| {
            let res = serve(&site, &Method::GET, path, &header::HeaderMap::new());
            let csp = header(&res, "content-security-policy").to_owned();
            csp.split(';')
                .map(str::trim)
                .find_map(|d| d.strip_prefix("frame-ancestors "))
                .map(str::to_owned)
        };
        for (path, rule) in [
            ("/", "*"),
            ("/map.html", "*"),
            ("/embed.html", "*"),
            ("/explore.html", "http://localhost:42102"),
            ("/explore.js", "'none'"),
        ] {
            assert_eq!(ancestors(path).as_deref(), Some(rule), "{path}");
        }
        let own = Site::new("https://viz.city.test", &live());
        let res = serve(
            &own,
            &Method::GET,
            "/render.html",
            &header::HeaderMap::new(),
        );
        let csp = header(&res, "content-security-policy");
        assert!(csp.ends_with("frame-ancestors *"), "{csp}");
    }

    /// A sandboxed page asks MapLibre's module and worker as `Origin: null`: the releases answer
    /// CORS, the pages and the explorer bundle (classic scripts, no CORS read) do not.
    #[test]
    fn the_releases_answer_cors_and_the_rest_does_not() {
        for path in [
            "/vendor/maplibre-gl-6.14.0.mjs",
            "/vendor/maplibre-gl-worker-6.14.0.mjs",
            "/vendor/pmtiles-4.5.0.js",
        ] {
            let res = get(path);
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(header(&res, "access-control-allow-origin"), "*", "{path}");
            assert!(
                res.headers()
                    .get("access-control-allow-credentials")
                    .is_none()
            );
        }
        for path in [
            "/explore.html",
            "/explore.js",
            "/explorer/explorer.js",
            "/x",
        ] {
            assert!(
                get(path)
                    .headers()
                    .get("access-control-allow-origin")
                    .is_none(),
                "{path}"
            );
        }
    }

    /// The explorer bundle goes out gzipped to a client that takes gzip and plain to one that
    /// does not, each with its own strong ETag from `SHA256SUMS`, and a reload is a 304.
    #[tokio::test]
    async fn the_explorer_bundle_is_gzipped_and_revalidates_to_304() {
        use std::io::Read;
        let plain = file("explorer/explorer.js").unwrap();
        let sum = include_str!("../../../../vendor/viz/SHA256SUMS")
            .lines()
            .find(|l| l.ends_with("  explorer/explorer.js"))
            .unwrap()
            .split_once("  ")
            .unwrap()
            .0;

        let res = get_with(
            "/explorer/explorer.js",
            &[(header::ACCEPT_ENCODING, "gzip, deflate, br, zstd")],
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, "content-encoding"), "gzip");
        assert_eq!(header(&res, "vary"), "Accept-Encoding");
        assert_eq!(header(&res, "cache-control"), "no-cache");
        assert_eq!(header(&res, "content-security-policy"), INERT);
        assert_eq!(header(&res, "x-content-type-options"), "nosniff");
        let etag = header(&res, "etag").to_owned();
        assert_eq!(etag, format!("\"{sum}-gzip\""));
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() * 3 < plain.len(),
            "{} of {}",
            body.len(),
            plain.len()
        );
        let mut unzipped = Vec::new();
        flate2::read::GzDecoder::new(&body[..])
            .read_to_end(&mut unzipped)
            .unwrap();
        assert!(unzipped == plain, "the gzip holds the summed bytes");

        for accept in ["", "identity", "gzip;q=0", "br", "*;q=0"] {
            let res = get_with(
                "/explorer/explorer.js",
                &[(header::ACCEPT_ENCODING, accept)],
            );
            assert!(res.headers().get("content-encoding").is_none(), "{accept}");
            assert_eq!(header(&res, "etag"), format!("\"{sum}\""), "{accept}");
            let body = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap();
            assert!(body == plain, "{accept}");
        }

        let again = get_with(
            "/explorer/explorer.js",
            &[
                (header::ACCEPT_ENCODING, "gzip"),
                (header::IF_NONE_MATCH, &format!("W/\"x\", {etag}")),
            ],
        );
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header(&again, "etag"), etag);
        assert_eq!(header(&again, "content-security-policy"), INERT);
        // The 304 describes the stored gzip representation it revalidates.
        assert_eq!(header(&again, "content-encoding"), "gzip");
        let body = axum::body::to_bytes(again.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
        // The gzip ETag never revalidates the plain body, nor a stale sum either.
        for (accept, tag) in [("identity", etag.as_str()), ("gzip", "\"0000\"")] {
            let res = get_with(
                "/explorer/explorer.js",
                &[
                    (header::ACCEPT_ENCODING, accept),
                    (header::IF_NONE_MATCH, tag),
                ],
            );
            assert_eq!(res.status(), StatusCode::OK, "{accept} {tag}");
        }

        let css = get_with(
            "/explorer/explorer.css",
            &[(header::ACCEPT_ENCODING, "gzip")],
        );
        assert_eq!(header(&css, "content-encoding"), "gzip");
        assert!(header(&css, "etag").ends_with("-gzip\""));
        // The pages carry the per-site meta: neither gzipped nor tagged here.
        let page = get_with("/explore.html", &[(header::ACCEPT_ENCODING, "gzip")]);
        assert!(page.headers().get("content-encoding").is_none());
        assert!(page.headers().get("etag").is_none());
    }

    #[tokio::test]
    async fn without_tiles_or_catalogue_nothing_is_named_for_them() {
        let site = Site::new(
            "http://h",
            &LiveAddresses {
                api: "https://api.x.test".into(),
                ..LiveAddresses::default()
            },
        );
        let res = serve(
            &site,
            &Method::GET,
            "/explore.html",
            &header::HeaderMap::new(),
        );
        let csp = header(&res, "content-security-policy").to_owned();
        assert!(csp.contains("connect-src https://api.x.test;"), "{csp}");
        assert!(csp.ends_with("frame-ancestors 'none'"), "{csp}");
        let map = serve(&site, &Method::GET, "/map.html", &header::HeaderMap::new());
        assert!(header(&map, "content-security-policy").contains("connect-src 'none';"));
    }

    #[tokio::test]
    async fn the_renderer_connects_nowhere_and_the_rest_is_inert() {
        let res = get("/render.html");
        assert!(header(&res, "content-security-policy").contains("connect-src 'none'"));
        for path in [
            "/explore.js",
            "/explorer/explorer.js",
            "/vendor/vega-6.4.0.min.js",
        ] {
            let res = get(path);
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(header(&res, "content-security-policy"), INERT, "{path}");
            assert_eq!(header(&res, "x-content-type-options"), "nosniff");
            assert!(
                header(&res, "content-type").starts_with("text/javascript"),
                "{path}"
            );
        }
        let module = get("/vendor/maplibre-gl-6.14.0.mjs");
        assert_eq!(header(&module, "content-encoding"), "gzip");
        assert!(header(&module, "cache-control").contains("immutable"));
        for path in [
            "/index.html",
            "/vendor/maplibre-gl-6.14.0.mjs.gz",
            "/../Cargo.toml",
            "/x.html",
            "/vendor/SHA256SUMS",
        ] {
            assert_eq!(get(path).status(), StatusCode::NOT_FOUND, "{path}");
        }
        let post = serve(
            &Site::new("http://h", &live()),
            &Method::POST,
            "/explore.html",
            &header::HeaderMap::new(),
        );
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
