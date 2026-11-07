//! The Portal's Prometheus surface (T-0463, OPS-16, TS-22).
//!
//! `components/monitoring` has scraped `/metrics` on this service since T-0038 and got the
//! single-page application back, which is a 200 with no series in it: a target that reports
//! healthy and measures nothing.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::telemetry;
use tower::ServiceExt;

async fn call(path: &str) -> (StatusCode, String, String) {
    let state = AppState::new(Config::for_tests(), None);
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("the Portal answers");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

#[tokio::test]
async fn the_scrape_target_answers_the_text_format_prometheus_reads() {
    call("/catalogue").await;
    let (status, content_type, body) = call("/metrics").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with(telemetry::TEXT_FORMAT),
        "{content_type}"
    );
    assert!(
        body.contains("# HELP jc_portal_requests_total requests answered by the Portal"),
        "{body}"
    );
}

#[tokio::test]
async fn a_request_through_the_surface_moves_the_counter() {
    call("/catalogue").await;

    let (_, _, body) = call("/metrics").await;
    let counted: Vec<&str> = body
        .lines()
        .filter(|line| line.starts_with("jc_portal_requests_total{"))
        // Every deep link of the single-page application is one route to the Portal.
        .filter(|line| line.contains("route=\"ui\""))
        .collect();
    assert!(!counted.is_empty(), "nothing counted the request:\n{body}");
    for line in &counted {
        let value: f64 = line
            .rsplit(' ')
            .next()
            .and_then(|number| number.parse().ok())
            .unwrap_or_default();
        assert!(value >= 1.0, "{line}");
    }
}

/// A scrape must not count itself, or the request rate becomes a function of the scrape
/// interval and says nothing about the traffic.
#[tokio::test]
async fn the_scrape_does_not_count_as_traffic() {
    call("/metrics").await;
    let (_, _, body) = call("/metrics").await;

    assert!(!body.contains("route=\"/metrics\""), "{body}");
}

/// The scrape carries no session and no CSRF token, so it must not sit behind either guard:
/// the monitor has neither and would scrape a 403 forever.
#[tokio::test]
async fn the_scrape_needs_no_session_and_no_csrf_token() {
    let (status, _, _) = call("/metrics").await;
    assert_eq!(status, StatusCode::OK);
}

/// The liveness probe runs every few seconds and is not traffic; counting it would make the
/// request rate mostly kubelet.
#[tokio::test]
async fn the_health_probe_does_not_count_as_traffic() {
    call("/api/v1/health").await;
    let (_, _, body) = call("/metrics").await;

    assert!(!body.contains("route=\"/api/v1/health\""), "{body}");
}

/// A duration has to arrive as a histogram, not as a summary: the dashboard reads latency as
/// `histogram_quantile` over these buckets, and a quantile computed inside one replica cannot
/// be combined with another replica's (docs Deployment/05 §1, T-0464).
#[tokio::test]
async fn a_duration_is_exported_as_buckets_a_dashboard_can_sum() {
    call("/catalogue").await;
    let (_, _, body) = call("/metrics").await;

    assert!(
        body.contains("# TYPE jc_portal_request_duration_seconds histogram"),
        "the duration is not a histogram:\n{body}"
    );
    assert!(
        body.lines().any(
            |line| line.starts_with("jc_portal_request_duration_seconds_bucket{")
                && line.contains("le=\"0.005\"")
        ),
        "no bucket in the low-millisecond range:\n{body}"
    );
    assert!(
        !body.contains("quantile=\""),
        "a summary quantile is exported and cannot be aggregated:\n{body}"
    );
}

#[tokio::test]
async fn run_timing_histograms_are_exported_with_profile_label() {
    telemetry::install();
    telemetry::record_run_timing("first_frame", "app-builder", 1200);
    telemetry::record_run_timing("first_version", "app-builder", 45_000);

    let (status, _, body) = call("/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("# TYPE jc_agent_run_first_frame_seconds histogram"),
        "first_frame histogram description missing:\n{body}"
    );
    assert!(
        body.contains("# TYPE jc_agent_run_first_version_seconds histogram"),
        "first_version histogram description missing:\n{body}"
    );
    assert!(
        body.contains("jc_agent_run_first_frame_seconds_bucket{")
            && body.contains("profile=\"app-builder\""),
        "first_frame histogram missing or missing profile label:\n{body}"
    );
    assert!(
        body.contains("jc_agent_run_first_version_seconds_bucket{")
            && body.contains("profile=\"app-builder\""),
        "first_version histogram missing or missing profile label:\n{body}"
    );
    assert!(
        body.lines().any(
            |line| line.starts_with("jc_agent_run_first_version_seconds_bucket{")
                && line.contains("le=\"60\"")
                && line.ends_with(" 1")
        ),
        "a 45 s first version does not land under the one-minute edge:\n{body}"
    );
}
