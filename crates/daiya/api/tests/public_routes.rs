#![forbid(unsafe_code)]
//! Public dashboards (R7, Grafana `pkg/services/publicdashboards`): opt-in records managed by
//! approvers, anonymous views stripped of queries and datasources, panel queries that run only
//! the saved targets inside capped ranges, and a per-address rate limit.

mod common;

use common::{World, dashboard, prometheus, world};
use daiya_api::PublicRateLimiter;
use http::StatusCode;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;

/// A dashboard with a `$job` variable (options `edge`, `core`), a range of one hour and a
/// panel whose saved target names it.
fn templated(uid: &str, ds: &str) -> Value {
    let mut d = dashboard(uid, ds);
    d["templating"] = json!({ "list": [{
        "name": "job", "type": "custom",
        "current": { "value": "edge", "text": "edge" },
        "options": [{ "value": "edge", "text": "edge" }, { "value": "core", "text": "core" }]
    }]});
    d["panels"][0]["targets"][0]["expr"] = json!("up{job=\"$job\"}");
    d["panels"][0]["transformations"] = json!([{ "id": "organize", "options": {} }]);
    d["panels"].as_array_mut().unwrap().push(json!({
        "id": 3, "type": "stat", "title": "Empty",
        "gridPos": { "x": 0, "y": 8, "w": 6, "h": 4 }
    }));
    d
}

async fn published(time_selection: bool) -> (World, String) {
    let w = world();
    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    w.add_dashboard(templated("pub", "prom"), None).await;
    let created = w
        .ok(
            "POST",
            "/api/dashboards/uid/pub/public-dashboards",
            Some(json!({ "uid": "pd-pub", "isEnabled": true, "timeSelectionEnabled": time_selection })),
        )
        .await;
    let token = created["accessToken"].as_str().unwrap().to_owned();
    (w, token)
}

#[tokio::test]
async fn approvers_manage_the_opt_in_record() {
    let w = world();
    w.add_dashboard(dashboard("d", "prom"), None).await;

    // Off by default; the raw token comes back once.
    let created = w
        .ok(
            "POST",
            "/api/dashboards/uid/d/public-dashboards",
            Some(json!({ "uid": "pd-1" })),
        )
        .await;
    assert_eq!(created["uid"], "pd-1");
    assert_eq!(created["isEnabled"], false);
    assert_eq!(created["accessToken"].as_str().unwrap().len(), 64);
    assert!(created.get("accessTokenHash").is_none());

    let got = w
        .ok("GET", "/api/dashboards/uid/d/public-dashboards", None)
        .await;
    assert_eq!(got["uid"], "pd-1");
    assert!(got.get("accessToken").is_none(), "{got}");
    let (st, _) = w
        .call(
            "GET",
            "/api/dashboards/uid/d/public-dashboards",
            Some("viewer"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = w
        .call(
            "GET",
            "/api/dashboards/uid/d/public-dashboards",
            Some("outsider"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call(
            "GET",
            "/api/dashboards/uid/none/public-dashboards",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Update every field through PATCH; the token is not handed out again.
    let updated = w
        .ok(
            "PATCH",
            "/api/dashboards/uid/d/public-dashboards/pd-1",
            Some(json!({
                "isEnabled": true, "timeSelectionEnabled": true, "annotationsEnabled": true,
                "showQueryText": true, "share": "email"
            })),
        )
        .await;
    assert_eq!(updated["isEnabled"], true);
    assert_eq!(updated["share"], "email");
    assert_eq!(updated["showQueryText"], true);
    assert!(updated.get("accessToken").is_none(), "{updated}");
    assert_eq!(updated["uid"], "pd-1");
    // Grafana loads the record by the path uid: another uid is 404 and changes nothing.
    let (st, _) = w
        .call(
            "PATCH",
            "/api/dashboards/uid/d/public-dashboards/pd-other",
            Some("admin"),
            Some(json!({ "isEnabled": false })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call(
            "PATCH",
            "/api/dashboards/uid/none/public-dashboards/pd-1",
            Some("admin"),
            Some(json!({ "isEnabled": false })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let still = w
        .ok("GET", "/api/dashboards/uid/d/public-dashboards", None)
        .await;
    assert_eq!(still["isEnabled"], true);

    // Editors and viewers cannot publish; another organization does not see the dashboard.
    for who in ["editor", "viewer", "outsider"] {
        let (st, _) = w
            .call(
                "POST",
                "/api/dashboards/uid/d/public-dashboards",
                Some(who),
                Some(json!({ "isEnabled": true })),
            )
            .await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
            "{who}: {st}"
        );
    }
    let (st, _) = w
        .call(
            "POST",
            "/api/dashboards/uid/none/public-dashboards",
            Some("admin"),
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Rotation needs approve too.
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/PublicDashboard.Rotate",
            Some("editor"),
            Some(json!({ "uid": "d" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let rotated = w
        .ok(
            "POST",
            "/odata/v4/PublicDashboard.Rotate",
            Some(json!({ "uid": "d" })),
        )
        .await;
    assert_ne!(rotated["accessToken"], created["accessToken"]);
}

#[tokio::test]
async fn the_anonymous_view_is_sanitized_and_answers_404_for_any_bad_token() {
    let (w, token) = published(false).await;
    let (st, view) = w
        .call(
            "GET",
            &format!("/api/public/dashboards/{token}"),
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{view}");
    assert_eq!(view["meta"]["canEdit"], false);
    assert_eq!(view["meta"]["publicDashboardEnabled"], true);
    assert_eq!(view["dashboard"]["timepicker"]["hidden"], true);
    let text = view.to_string();
    assert!(!text.contains("\"prom\""), "no datasource uid: {text}");
    assert!(!text.contains("up{"), "no query text: {text}");
    assert!(view["dashboard"].get("id").is_none());

    for bad in ["short", &"z".repeat(64), &"0".repeat(64)] {
        let (st, _) = w
            .call("GET", &format!("/api/public/dashboards/{bad}"), None, None)
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{bad}");
    }

    // Disabled: the same 404.
    w.ok(
        "PATCH",
        "/api/dashboards/uid/pub/public-dashboards/pd-pub",
        Some(json!({ "isEnabled": false })),
    )
    .await;
    let (st, _) = w
        .call(
            "GET",
            &format!("/api/public/dashboards/{token}"),
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call(
            "POST",
            &format!("/api/public/dashboards/{token}/panels/1/query"),
            None,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn query_text_is_shown_only_when_the_owner_allows_it() {
    let (w, token) = published(true).await;
    w.ok(
        "PATCH",
        "/api/dashboards/uid/pub/public-dashboards/pd-pub",
        Some(json!({ "showQueryText": true })),
    )
    .await;
    let (_, view) = w
        .call(
            "GET",
            &format!("/api/public/dashboards/{token}"),
            None,
            None,
        )
        .await;
    assert!(view.to_string().contains("up{"), "{view}");
    assert!(!view.to_string().contains("\"prom\""), "{view}");
    assert_eq!(view["dashboard"]["timepicker"]["hidden"], false);
}

#[tokio::test]
async fn panel_queries_run_only_the_saved_targets() {
    let (w, token) = published(true).await;
    let q = |panel: u32| format!("/api/public/dashboards/{token}/panels/{panel}/query");

    let (st, answer) = w.call("POST", &q(1), None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{answer}");
    assert!(answer["results"]["A"]["frames"].is_array(), "{answer}");
    // No body at all is the same request.
    let (st, _) = w.call("POST", &q(1), None, None).await;
    assert_eq!(st, StatusCode::OK);

    for key in [
        "targets",
        "queries",
        "datasource",
        "expr",
        "rawSql",
        "expression",
    ] {
        let (st, body) = w
            .call("POST", &q(1), None, Some(json!({ key: "up" })))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{key}: {body}");
    }
    let (st, _) = w
        .call(
            "POST",
            &q(1),
            None,
            Some(json!({ "variables": { "other": "x" } })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = w
        .call(
            "POST",
            &q(1),
            None,
            Some(json!({ "variables": { "job": "admin\"}" } })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, body) = w
        .call(
            "POST",
            &q(1),
            None,
            Some(json!({ "variables": { "job": "core" } })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Time selection on: a caller range is used, an inverted one refused, a wide one capped.
    let (st, _) = w
        .call(
            "POST",
            &q(1),
            None,
            Some(json!({ "timeRange": { "from": "1700000000000", "to": "1700000600000" } })),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = w
        .call(
            "POST",
            &q(1),
            None,
            Some(json!({ "timeRange": { "from": "1700000600000", "to": "1700000000000" } })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = w
        .call(
            "POST",
            &q(1),
            None,
            Some(
                json!({ "timeRange": { "from": "0", "to": "1700000600000" }, "maxDataPoints": 0 }),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = w
        .call("POST", &q(1), None, Some(json!({ "intervalMs": 0 })))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = w.call("POST", &q(9), None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w.call("POST", &q(3), None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a panel without queries");
    let (st, _) = w
        .call(
            "POST",
            "/api/public/dashboards/nothex/panels/1/query",
            None,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_repeated_query_is_served_from_the_public_cache() {
    let w = world();
    let (url, seen) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    let mut d = dashboard("cached", "prom");
    d["time"] = json!({ "from": "1700000000000", "to": "1700000600000" });
    w.add_dashboard(d, None).await;
    let created = w
        .ok(
            "POST",
            "/api/dashboards/uid/cached/public-dashboards",
            Some(json!({ "isEnabled": true })),
        )
        .await;
    let token = created["accessToken"].as_str().unwrap();
    let uri = format!("/api/public/dashboards/{token}/panels/1/query");
    let (st, first) = w.call("POST", &uri, None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{first}");
    let (_, second) = w.call("POST", &uri, None, Some(json!({}))).await;
    assert_eq!(first, second);
    assert_eq!(
        seen.lock().len(),
        1,
        "the second answer came from the cache"
    );
}

#[tokio::test]
async fn the_rate_limit_answers_429_per_address() {
    let (mut w, token) = published(false).await;
    w.state.public_rate_limiter = Arc::new(PublicRateLimiter::new(2, 60));
    w.app = daiya_api::api_router(
        w.state.clone(),
        jc_router::Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    let uri = format!("/api/public/dashboards/{token}");
    for _ in 0..2 {
        let (st, _) = w.call("GET", &uri, None, None).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, body) = w.call("GET", &uri, None, None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    let (st, _) = w
        .call(
            "POST",
            &format!("{uri}/panels/1/query"),
            None,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn the_limiter_resets_after_its_window_and_bounds_its_table() {
    let a: IpAddr = "203.0.113.1".parse().unwrap();
    let b: IpAddr = "203.0.113.2".parse().unwrap();
    let limiter = PublicRateLimiter::new(1, 1);
    limiter.check_rate_limit(a).unwrap();
    assert!(limiter.check_rate_limit(a).is_err());
    limiter.check_rate_limit(b).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    limiter.check_rate_limit(a).unwrap();

    // A full table refuses a new address while every window is live, and frees expired ones.
    // Filling 65,536 addresses takes over a second on a CI runner, so the window is 5 s.
    let full = PublicRateLimiter::new(5, 5);
    for i in 0..PublicRateLimiter::MAX_TRACKED_ADDRESSES {
        full.check_rate_limit(IpAddr::V6(Ipv6Addr::from(i as u128)))
            .unwrap();
    }
    let newcomer = IpAddr::V6(Ipv6Addr::from(u128::MAX));
    assert!(full.check_rate_limit(newcomer).is_err());
    full.check_rate_limit(IpAddr::V6(Ipv6Addr::from(7u128)))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5100));
    full.check_rate_limit(newcomer).unwrap();
}

#[tokio::test]
async fn store_failures_are_errors() {
    let (w, token) = published(false).await;
    let view = format!("/api/public/dashboards/{token}");
    let query = format!("{view}/panels/1/query");
    let cases: &[(&'static str, &str, &str, Option<Value>)] = &[
        ("get_public_dashboard_by_token_hash", "GET", &view, None),
        (
            "get_public_dashboard_by_token_hash",
            "POST",
            &query,
            Some(json!({})),
        ),
        ("get_quotas", "POST", &query, Some(json!({}))),
        (
            "get_public_dashboard_by_dashboard_uid",
            "GET",
            "/api/dashboards/uid/pub/public-dashboards",
            None,
        ),
        (
            "get_public_dashboard_by_dashboard_uid",
            "POST",
            "/api/dashboards/uid/pub/public-dashboards",
            Some(json!({})),
        ),
        (
            "get_dashboard",
            "POST",
            "/api/dashboards/uid/pub/public-dashboards",
            Some(json!({})),
        ),
        (
            "upsert_public_dashboard",
            "POST",
            "/api/dashboards/uid/pub/public-dashboards",
            Some(json!({})),
        ),
        (
            "get_public_dashboard_by_dashboard_uid",
            "PATCH",
            "/api/dashboards/uid/pub/public-dashboards/pd-pub",
            Some(json!({})),
        ),
        (
            "get_public_dashboard",
            "POST",
            "/odata/v4/PublicDashboard.Rotate",
            Some(json!({ "uid": "pub" })),
        ),
        (
            "get_public_dashboard_by_dashboard_uid",
            "POST",
            "/odata/v4/PublicDashboard.Rotate",
            Some(json!({ "uid": "pub" })),
        ),
    ];
    for (method_name, method, uri, body) in cases {
        w.store.fail(method_name);
        let (st, answer) = w.call(method, uri, Some("admin"), body.clone()).await;
        w.store.heal();
        assert_eq!(
            st,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method_name}: {method} {uri}: {answer}"
        );
    }
    // A new record whose write fails.
    w.add_dashboard(dashboard("fresh", "prom"), None).await;
    w.store.fail("upsert_public_dashboard");
    let (st, _) = w
        .call(
            "POST",
            "/api/dashboards/uid/fresh/public-dashboards",
            Some("admin"),
            Some(json!({})),
        )
        .await;
    w.store.heal();
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
}

/// A caller `from` near i64::MIN must not wrap the span and skip the public range cap: the query
/// still starts inside the capped window, not in 1970.
#[tokio::test]
async fn an_overflowing_public_range_is_still_capped() {
    let w = world();
    let (url, seen) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    w.add_dashboard(templated("pub", "prom"), None).await;
    let created = w
        .ok(
            "POST",
            "/api/dashboards/uid/pub/public-dashboards",
            Some(json!({ "uid": "pd-pub", "isEnabled": true, "timeSelectionEnabled": true })),
        )
        .await;
    let token = created["accessToken"].as_str().unwrap();
    let (st, body) = w
        .call(
            "POST",
            &format!("/api/public/dashboards/{token}/panels/1/query"),
            None,
            Some(json!({ "timeRange": { "from": "-9223372036854775808", "to": "1700000600000" } })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let head = seen.lock().last().cloned().unwrap();
    let start: f64 = head
        .split(['?', '&', ' '])
        .find_map(|p| p.strip_prefix("start="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(start > 1_690_000_000.0, "capped start, not 1970: {head}");
}
