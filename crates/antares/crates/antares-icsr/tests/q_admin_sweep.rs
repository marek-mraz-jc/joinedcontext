// SPDX-License-Identifier: EUPL-1.2
//! T-3855: every route `/q` serves in a `views,internal-csr` build refuses a
//! caller without the admin secret (none, a wrong one, the bare secret) and
//! serves one with it; health and readiness stay open, `/q/metrics` follows
//! `metrics_open`. [`ROUTES`] is proved equal to the routes the sources
//! mount, and every method is tried on every path, so a route or a method
//! added after the guard (`route_layer`) turns this red.

use antares_ops::test_kit::{AdminAuth, TestBroker};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

const SECRET: &str = "the-q-sweep-secret-of-at-least-32-bytes";

const METHODS: [&str; 5] = ["GET", "PUT", "POST", "PATCH", "DELETE"];

/// Every guarded `/q` route: its template as mounted, a concrete path, and
/// the methods it serves.
const ROUTES: &[(&str, &str, &[&str])] = &[
    ("/metrics", "/q/metrics", &["GET"]),
    ("/tenants", "/q/tenants", &["GET"]),
    ("/tenants/{tenant}", "/q/tenants/acme", &["GET", "DELETE"]),
    (
        "/tenants/{tenant}/digest",
        "/q/tenants/acme/digest",
        &["GET"],
    ),
    (
        "/tenants/{tenant}/export",
        "/q/tenants/acme/export",
        &["GET"],
    ),
    (
        "/tenants/{tenant}/import",
        "/q/tenants/acme/import",
        &["POST"],
    ),
    (
        "/tenants/{tenant}/types/{type}/attributes",
        "/q/tenants/acme/types/Room/attributes",
        &["GET"],
    ),
    ("/dead-letters", "/q/dead-letters", &["GET"]),
    (
        "/dead-letters/{id}",
        "/q/dead-letters/urn:dl:1",
        &["DELETE"],
    ),
    (
        "/dead-letters/{id}/replay",
        "/q/dead-letters/urn:dl:1/replay",
        &["POST"],
    ),
    ("/views", "/q/views", &["GET"]),
    ("/views/{id}", "/q/views/cars", &["GET", "PUT", "DELETE"]),
    ("/views/{id}/preview", "/q/views/cars/preview", &["POST"]),
    (
        "/schemas/{tenant}",
        "/q/schemas/acme",
        &["GET", "PUT", "DELETE"],
    ),
    (
        "/retention/{tenant}",
        "/q/retention/acme",
        &["GET", "PUT", "DELETE"],
    ),
    (
        "/retention/{tenant}/preview",
        "/q/retention/acme/preview",
        &["POST"],
    ),
    (
        "/retention/{tenant}/deleted",
        "/q/retention/acme/deleted",
        &["GET"],
    ),
    ("/policy", "/q/policy", &["GET"]),
    (
        "/policy/{tenant}",
        "/q/policy/acme",
        &["GET", "PUT", "DELETE"],
    ),
    ("/grants", "/q/grants", &["GET"]),
    (
        "/grants/{target}/{source}",
        "/q/grants/b/a",
        &["GET", "PUT", "DELETE"],
    ),
];

/// Guarded `/q` routes of a surface this crate may not link (module rule
/// surface-dev): the template, and the file and test of that surface that
/// sweeps them as [`every_q_route_refuses_without_the_secret_and_serves_with_it`] does.
const PROVED_ELSEWHERE: &[(&str, &str, &str)] = &[(
    "/mcp-confirmations/{id}",
    "crates/antares-mcp/tests/mcp_decided.rs",
    "the_decision_routes_refuse_a_caller_without_the_admin_secret",
)];

const OPEN: [&str; 2] = ["/q/health", "/q/ready"];

async fn app(metrics_open: bool) -> axum::Router {
    let selfs = antares_icsr::SelfUrls::parse("http://self.invalid").unwrap_or_default();
    let auth = AdminAuth::new(SECRET, metrics_open).unwrap_or_else(|e| unreachable!("{e}"));
    let broker = TestBroker::new()
        .await
        .and_then(|b| b.with_forward_transport(antares_icsr::transport(selfs)))
        .map_or_else(|e| unreachable!("{e}"), |b| b.with_admin_auth(auth));
    broker
        .serve_mounted(Vec::new())
        .unwrap_or_else(|e| unreachable!("{e}"))
}

async fn status(app: &axum::Router, method: &str, path: &str, auth: Option<&str>) -> StatusCode {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        // the bounds wall answers a bodyless write without a length with 411
        .header(header::CONTENT_LENGTH, "0");
    if let Some(a) = auth {
        req = req.header(header::AUTHORIZATION, a);
    }
    let req = req
        .body(Body::empty())
        .unwrap_or_else(|e| unreachable!("{e}"));
    app.clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e| unreachable!("{e}"))
        .status()
}

#[tokio::test]
async fn every_q_route_refuses_without_the_secret_and_serves_with_it() {
    let app = app(false).await;
    let good = format!("Bearer {SECRET}");
    for (_, path, served) in ROUTES {
        for method in METHODS {
            for bad in [None, Some("Bearer not-the-secret"), Some(SECRET)] {
                let got = status(&app, method, path, bad).await;
                if served.contains(&method) {
                    assert_eq!(got, StatusCode::UNAUTHORIZED, "{method} {path} {bad:?}");
                } else {
                    // a method no route serves may be told so; never served
                    assert!(
                        matches!(
                            got,
                            StatusCode::UNAUTHORIZED | StatusCode::METHOD_NOT_ALLOWED
                        ),
                        "{method} {path} {bad:?}: {got}"
                    );
                }
            }
            let with = status(&app, method, path, Some(&good)).await;
            if served.contains(&method) {
                assert!(
                    !matches!(
                        with,
                        StatusCode::UNAUTHORIZED | StatusCode::METHOD_NOT_ALLOWED
                    ),
                    "{method} {path} with the secret: {with}"
                );
            } else {
                assert_eq!(
                    with,
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{method} {path} is served: add it to ROUTES"
                );
            }
        }
    }
}

#[tokio::test]
async fn health_and_readiness_stay_open_and_metrics_follows_metrics_open() {
    let closed = app(false).await;
    for path in OPEN {
        assert_eq!(
            status(&closed, "GET", path, None).await,
            StatusCode::OK,
            "{path}"
        );
    }
    assert_eq!(
        status(&closed, "GET", "/q/metrics", None).await,
        StatusCode::UNAUTHORIZED
    );
    let open = app(true).await;
    assert_ne!(
        status(&open, "GET", "/q/metrics", None).await,
        StatusCode::UNAUTHORIZED
    );
    // metrics_open opens the scrape and nothing else
    assert_eq!(
        status(&open, "GET", "/q/views", None).await,
        StatusCode::UNAUTHORIZED
    );
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace().join(rel)).unwrap_or_else(|e| unreachable!("{rel}: {e}"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// The string literals that open each `.route(` call of `src`; `None` when
/// one opens with anything else, which this proof cannot read.
fn route_literals(src: &str) -> Option<Vec<String>> {
    src.split(".route(")
        .skip(1)
        .map(|rest| {
            let rest = rest.trim_start().strip_prefix('"')?;
            rest.split_once('"').map(|(p, _)| p.to_owned())
        })
        .collect()
}

/// `ROUTES` is what the sources mount: the admin surface's `PATHS`, and the
/// routes of every module that adds to it, found by its `set_admin` or
/// `add_admin` call. A new module, or a route in one, turns this red.
#[test]
fn routes_equals_what_the_sources_mount() {
    let mut files = Vec::new();
    rust_files(&workspace().join("crates"), &mut files);
    let mut adders = BTreeSet::new();
    for f in files {
        let rel = f
            .strip_prefix(workspace())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !rel.contains("/src/") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        for (at, _) in text.match_indices("_admin(") {
            let head = text.get(..at).unwrap_or_default();
            let call = head.ends_with("set") || head.ends_with("add");
            let def = head.ends_with("fn set") || head.ends_with("fn add");
            if call && !def {
                adders.insert(rel.clone());
            }
        }
    }
    assert_eq!(
        adders,
        BTreeSet::from([
            "crates/antares-broker/src/main.rs".to_owned(),
            "crates/antares-icsr/src/lib.rs".to_owned(),
            "crates/antares-ops/src/lib.rs".to_owned(),
            "crates/antares-ops/src/views.rs".to_owned(),
        ]),
        "a new module adds /q routes: list its route file below and its routes in ROUTES"
    );

    let api = read("crates/antares-api/src/ops.rs");
    let paths = api
        .split_once("const PATHS:")
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(block, _)| block)
        .unwrap_or_default();
    let mut mounted: BTreeSet<String> = paths
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    // `Admin::router` mounts `PATHS` through the names it destructures them
    // into (`*_p`); a literal there is a route `PATHS` does not hold, and a
    // merge or a nest is one more router this proof has not read.
    let body = api
        .split_once("impl ApiSurface for Admin")
        .and_then(|(_, rest)| rest.split_once("fn version_info"))
        .map(|(body, _)| body)
        .unwrap_or_default();
    for arg in body.split(".route(").skip(1) {
        let arg = arg.trim_start();
        match arg.strip_prefix('"').and_then(|a| a.split_once('"')) {
            Some((literal, _)) => {
                mounted.insert(literal.to_owned());
            }
            None => assert!(
                arg.split_once(',')
                    .is_some_and(|(name, _)| name.ends_with("_p")),
                "Admin::router mounts a path not from PATHS: {arg:.40}"
            ),
        }
    }
    assert_eq!(
        (
            body.matches(".merge(").count(),
            body.matches(".nest(").count()
        ),
        (2, 0),
        "Admin::router merges another router: add its routes here and to ROUTES"
    );
    for rel in [
        "crates/antares-ops/src/views.rs",
        "crates/antares-ops/src/views/preview.rs",
        "crates/antares-ops/src/views/rule_documents.rs",
        "crates/antares-ops/src/views/retention.rs",
        "crates/antares-icsr/src/grant.rs",
        "crates/antares-mcp/src/decided.rs",
    ] {
        let found = route_literals(&read(rel))
            .unwrap_or_else(|| unreachable!("{rel}: a .route( without a path literal"));
        assert!(!found.is_empty(), "{rel} mounts no route");
        mounted.extend(found);
    }
    let mut listed: BTreeSet<String> = ROUTES.iter().map(|(t, _, _)| (*t).to_owned()).collect();
    listed.extend(OPEN.map(|p| p.trim_start_matches("/q").to_owned()));
    for (template, rel, test) in PROVED_ELSEWHERE {
        assert!(
            read(rel).contains(&format!("async fn {test}(")),
            "{template}: {rel} lost its admin sweep {test}"
        );
        listed.insert((*template).to_owned());
    }
    assert_eq!(listed, mounted);
}
