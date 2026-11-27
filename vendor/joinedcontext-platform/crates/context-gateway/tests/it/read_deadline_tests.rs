//! T-4903: a plain read holds its slot for as long as the broker takes, so a read past
//! `SLOW_AFTER` weighs more against its caller's share and the gateway's slow budget, and one past
//! the deadline is answered 503 with a problem document and `Retry-After`, its broker call
//! dropped with it. A quick read of another caller still gets through.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::any;
use axum::Router;
use context_gateway::app::{router, Gateway};
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::Endpoint;
use jc_core::kinds::{Audience, PolicySpec, Representation};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

const SLUG: &str = "k7r2m4xq9vbn3tdw6hcy5pajfe";
const SPACE: &str = "ovzdusie";
const DOMAIN: &str = "banskabystrica.sk";
// The deployed timings: the gateway takes no other (a test-only knob is dead code, T-4903).
const SLOW_AFTER: Duration = context_gateway::middleware::in_flight::SLOW_AFTER;
const DEADLINE: Duration = context_gateway::middleware::in_flight::READ_DEADLINE;

/// Counts the broker calls in progress; one comes off when its handler is dropped, answered or not.
struct Live(Arc<AtomicUsize>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A broker that answers `type=Fast` at once with an empty page and never answers anything else.
async fn broker() -> (String, Arc<AtomicUsize>) {
    let live = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&live);
    let app = Router::new().fallback(any(move |request: Request| {
        let counter = Arc::clone(&counter);
        async move {
            if request
                .uri()
                .query()
                .is_some_and(|q| q.contains("type=Fast"))
            {
                return ([("content-type", "application/json")], "[]");
            }
            counter.fetch_add(1, Ordering::SeqCst);
            let _live = Live(counter);
            std::future::pending::<()>().await;
            ([("content-type", "application/json")], "[]")
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), live)
}

fn endpoint() -> Endpoint {
    let policy: PolicySpec = serde_norway::from_str(&format!(
        "contextSpaceRef: {SPACE}\n\
         assigner: did:web:{DOMAIN}\n\
         assignee: {{ kind: role, id: public }}\n\
         operations: [queryEntity]\n\
         information:\n\
         \x20 - entities:\n\
         \x20     - type: Slow\n\
         \x20     - type: Fast\n"
    ))
    .expect("the policy spec parses");
    Endpoint {
        declared_types: None,
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        open_data: false,
        roles: Default::default(),
        slug: SLUG.to_owned(),
        title: Default::default(),
        description: Default::default(),
        space: SPACE.to_owned(),
        tenant: context_gateway::middleware::tenancy::Tenant::segment(SPACE).ok(),
        project: SPACE.to_owned(),
        audience: Audience::Public,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: Default::default(),
        projection: None,
        view_mapping: None,
        base_path: format!("/api/endpoint/{SLUG}"),
        models: Vec::new(),
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![policy],
    }
}

fn read(kind: &str, address: &str) -> Request<Body> {
    Request::builder()
        .uri(format!(
            "/api/endpoint/{SLUG}/ngsi-ld/v1/entities?type={kind}&limit=1"
        ))
        .header("x-forwarded-for", address)
        .body(Body::empty())
        .expect("a request")
}

fn gateway(url: &str, total: usize, per_caller: usize) -> Router {
    router(Arc::new(
        Gateway::new(Broker::new(url.to_owned()), Box::new(PolicyPdp), DOMAIN)
            .limit_in_flight(total, per_caller, 1)
            .serve([endpoint()]),
    ))
}

async fn problem(answer: axum::response::Response) -> (String, serde_json::Value) {
    assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
    let retry = answer.headers()["retry-after"]
        .to_str()
        .expect("a number")
        .to_owned();
    let body = axum::body::to_bytes(answer.into_body(), 1 << 16)
        .await
        .expect("the body");
    (
        retry,
        serde_json::from_slice(&body).expect("a problem document"),
    )
}

#[tokio::test]
async fn a_read_past_the_deadline_is_answered_503_and_its_broker_call_dropped() {
    let (url, live) = broker().await;
    let app = gateway(&url, 16, 8);
    let started = Instant::now();
    let answer = app
        .oneshot(read("Slow", "192.0.2.1"))
        .await
        .expect("an answer");
    let took = started.elapsed();
    let (retry, problem) = problem(answer).await;
    assert_eq!(retry, "10");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("longer than")),
        "{problem}"
    );
    assert!(
        took >= DEADLINE && took < DEADLINE * 4,
        "answered after {took:?}"
    );
    // The gateway dropped its upstream request, so the broker's handler went away too.
    let until = Instant::now() + Duration::from_secs(5);
    while live.load(Ordering::SeqCst) > 0 {
        assert!(
            Instant::now() < until,
            "the broker call outlived the deadline"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn slow_reads_weigh_more_and_leave_room_for_another_callers_quick_read() {
    let (url, live) = broker().await;
    // Of a slow budget of 16, anonymous reads weigh on 8 (T-5124): two slow reads; a caller's
    // share of 8 holds two slow reads too.
    let app = gateway(&url, 32, 8);
    let slow: Vec<_> = (0..3)
        .map(|_| tokio::spawn(app.clone().oneshot(read("Slow", "192.0.2.1"))))
        .collect();
    tokio::time::sleep(SLOW_AFTER + Duration::from_millis(500)).await;

    // Two slow reads weigh 8: the caller's own quick read no longer fits its share.
    let (retry, _) = problem(
        app.clone()
            .oneshot(read("Fast", "192.0.2.1"))
            .await
            .expect("an answer"),
    )
    .await;
    assert_eq!(retry, "1");
    // Another caller's quick read is served while they run.
    let started = Instant::now();
    let other = app
        .clone()
        .oneshot(read("Fast", "192.0.2.2"))
        .await
        .expect("an answer");
    assert_eq!(other.status(), StatusCode::OK);
    assert!(started.elapsed() < SLOW_AFTER, "the quick read waited");

    let mut refused_slow = 0;
    let mut ran_out = 0;
    for answer in slow {
        let (retry, problem) = problem(answer.await.expect("joined").expect("answered")).await;
        let detail = problem["detail"].as_str().unwrap_or_default().to_owned();
        if detail.contains("slow reads") {
            assert_eq!(retry, "1");
            refused_slow += 1;
        } else {
            assert!(detail.contains("longer than"), "{problem}");
            ran_out += 1;
        }
    }
    assert_eq!(
        (refused_slow, ran_out),
        (1, 2),
        "the third slow read found no room"
    );
    let until = Instant::now() + Duration::from_secs(5);
    while live.load(Ordering::SeqCst) > 0 {
        assert!(Instant::now() < until, "a broker call outlived its read");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// T-5124: anonymous requests share a part of the slots; once it is held another anonymous request
/// is answered 429 with `Retry-After` and never reaches the broker.
#[tokio::test]
async fn an_anonymous_flood_past_its_share_is_answered_429() {
    let (url, live) = broker().await;
    let app = gateway(&url, 8, 8);
    let share = context_gateway::middleware::in_flight::anonymous_share(8);
    let held: Vec<_> = (0..share)
        .map(|n| tokio::spawn(app.clone().oneshot(read("Slow", &format!("flood-{n}")))))
        .collect();
    let until = Instant::now() + Duration::from_secs(5);
    while live.load(Ordering::SeqCst) < share {
        assert!(Instant::now() < until, "the broker never held the share");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let answer = app
        .clone()
        .oneshot(read("Slow", "one-too-many"))
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(answer.headers()["retry-after"], "1");
    assert_eq!(live.load(Ordering::SeqCst), share, "nothing more was sent");
    for read in held {
        read.abort();
    }
}

/// T-5921: the load generator's verified token reads only; its write is answered 403 before the
/// broker sees it, and a plain caller's write meets no such rule.
#[tokio::test]
async fn a_verified_load_generator_may_only_read() {
    let (url, live) = broker().await;
    let realm = crate::common::Realm::new();
    let app = router(Arc::new(
        Gateway::new(Broker::new(url), Box::new(PolicyPdp), DOMAIN)
            .limit_in_flight(16, 8, 1)
            .serve_loadtest(Some("perf-loadtest".to_owned()), 8)
            .serve([endpoint()])
            .authenticate(
                Arc::new(realm.verifier()),
                context_gateway::auth::accounts::ServiceAccounts::new(),
                None,
            ),
    ));
    let write = |token: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/endpoint/{SLUG}/ngsi-ld/v1/entities"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(r#"{"id":"urn:ngsi-ld:Slow:x","type":"Slow"}"#))
            .expect("a request")
    };
    let load = realm.workload_token("perf-loadtest", serde_json::json!(SLUG));
    let answer = app.clone().oneshot(write(&load)).await.expect("an answer");
    assert_eq!(answer.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(answer.into_body(), 1 << 16)
        .await
        .expect("the body");
    let problem: serde_json::Value = serde_json::from_slice(&body).expect("a problem");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("load generator")),
        "{problem}"
    );
    assert_eq!(live.load(Ordering::SeqCst), 0, "the broker saw nothing");

    let other = realm.workload_token("another-client", serde_json::json!(SLUG));
    let answer = app.oneshot(write(&other)).await.expect("an answer");
    let body = axum::body::to_bytes(answer.into_body(), 1 << 16)
        .await
        .expect("the body");
    assert!(
        !String::from_utf8_lossy(&body).contains("load generator"),
        "another client's write meets the Policy, not the generator's rule"
    );
}
