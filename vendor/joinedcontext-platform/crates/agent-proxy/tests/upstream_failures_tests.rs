//! What the proxy answers when the realm, the Portal or the gateway fails it (AG-52, T-2418,
//! T-7795): a connection closed without a word, an answer that breaks off, a token that is not
//! the Portal's, a run the Portal does not hold. A failure of the platform is a `503` or a `502`
//! that says which part failed; a refusal of the caller is a `401`, `403` or `404`; and every
//! one of them is a log line an operator can read, never with a token in it.

mod common;
mod raw_upstream;

use agent_proxy::router as router_of;
use agent_proxy::ProxyState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{authed, body_of, sample_run, state, Bases, PROXY_CLIENT, RUN_ID};
use raw_upstream::{broken, reply, Answer, RawUpstream};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const PORTAL_TOKEN: &str = "portal-service-account-token";
const NOT_PORTAL: &str = "a-persons-token-of-the-portal-client";
const PERSON_TOKEN: &str = "person-access-token";
const REFRESH: &str = "delegated-refresh-token";
/// A run id of the right shape that the Portal does not hold.
const OTHER_RUN: &str = "aaaaaaaa-98fc-1c14-9afb-4c7b2756a120";

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every log line of this binary so far (one global subscriber: a per-test one misses callsites
/// first hit on another thread, see edge_delegation_tests.rs).
fn logged() -> String {
    static LINES: std::sync::OnceLock<Lines> = std::sync::OnceLock::new();
    let lines = LINES.get_or_init(|| {
        let lines = Lines::default();
        let writer = lines.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish(),
        )
        .expect("the suite's subscriber is the only one");
        lines
    });
    String::from_utf8_lossy(&lines.0.lock().expect("the lines")).into_owned()
}

/// Waits up to two seconds for a line a spawned task writes.
async fn logged_eventually(needle: &str) -> String {
    for _ in 0..40 {
        let lines = logged();
        if lines.contains(needle) {
            return lines;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    logged()
}

/// The realm's introspection: the Portal's service account, a person's token of the Portal's
/// client (the same `azp`, not its service account), and the run's person.
fn introspection(body: &str) -> Answer {
    if body.contains(&format!("token={PORTAL_TOKEN}")) {
        return reply(
            200,
            &format!(
                r#"{{"active":true,"azp":"portal-api","username":"service-account-portal-api","aud":["{PROXY_CLIENT}"]}}"#
            ),
        );
    }
    if body.contains(&format!("token={NOT_PORTAL}")) {
        return reply(
            200,
            &format!(
                r#"{{"active":true,"azp":"portal-api","username":"someone","aud":["{PROXY_CLIENT}"]}}"#
            ),
        );
    }
    reply(
        200,
        r#"{"active":true,"azp":"edge","username":"demo.steward@hel.fi"}"#,
    )
}

/// A realm that answers every introspection, mints the proxy's own token, and leaves the
/// exchange and the refresh to `rest`.
async fn realm(rest: impl Fn(&str) -> Answer + Send + Sync + 'static) -> RawUpstream {
    RawUpstream::start(move |path, body| {
        if path.ends_with("/introspect") {
            return introspection(body);
        }
        if body.contains("grant_type=client_credentials") {
            return reply(
                200,
                r#"{"access_token":"proxy-own-token","expires_in":300}"#,
            );
        }
        rest(body)
    })
    .await
}

/// A Portal that holds the sample run (`active`) or answers its lookup with `status`.
async fn portal(status: u16) -> RawUpstream {
    let run = sample_run(false);
    let record = serde_json::json!({
        "id": run.id, "project": run.project, "appName": run.app_name,
        "endpointSlug": run.endpoint_slug, "allowsWrite": run.allows_write,
        "branch": run.branch, "pathPrefix": run.path_prefix, "status": "building",
        "ticketHash": run.ticket_hash, "maxTokens": run.max_tokens,
        "allowedHosts": run.allowed_hosts, "requestsPerMinute": run.requests_per_minute,
        "maxResponseBytes": run.max_response_bytes, "createdBy": run.created_by,
        "modelName": run.model_name,
    })
    .to_string();
    RawUpstream::start(move |_, _| match status {
        200 => reply(200, &record),
        other => reply(other, "{}"),
    })
    .await
}

fn bases(realm: Option<&RawUpstream>, portal: Option<&RawUpstream>) -> Bases {
    Bases {
        realm: realm.map(|r| format!("{}/realms/jc", r.base)),
        portal: portal.map_or(Bases::default().portal, |p| p.base.clone()),
        ..Bases::default()
    }
}

fn internal(run: &str, door: &str, bearer: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/internal/runs/{run}/{door}"))
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .expect("a request")
}

fn hand_over_body() -> String {
    serde_json::json!({ "subjectToken": PERSON_TOKEN }).to_string()
}

/// The Portal's client is not the Portal: a person's token of it is refused and logged with the
/// door it knocked on; a realm that cannot judge the token is a `503` that says so.
#[tokio::test]
async fn an_internal_call_not_from_the_portal_or_unjudged_is_refused_with_whose_fault() {
    logged();
    let realm = realm(|_| reply(200, "{}")).await;
    let refused = router_of(state(sample_run(false), bases(Some(&realm), None)))
        .oneshot(internal(RUN_ID, "identity", NOT_PORTAL, &hand_over_body()))
        .await
        .expect("an answer");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let lines = logged();
    assert!(
        lines.contains("an internal call not from the Portal's service account"),
        "{lines}"
    );
    assert!(
        lines.contains("route=\"only the Portal hands a run the identity of its person\""),
        "{lines}"
    );
    assert!(!lines.contains(NOT_PORTAL), "a token reached a log line");

    let silent = RawUpstream::start(|_, _| None).await;
    let unjudged = router_of(state(sample_run(false), bases(Some(&silent), None)))
        .oneshot(internal(RUN_ID, "data-credential", PORTAL_TOKEN, ""))
        .await
        .expect("an answer");
    assert_eq!(unjudged.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(body_of(unjudged)
        .await
        .contains("the realm could not be reached"));
}

/// A hand-over whose body is not `{"subjectToken": …}` is a `400` that names the shape.
#[tokio::test]
async fn a_hand_over_body_of_another_shape_names_the_one_it_takes() {
    let realm = realm(|_| reply(200, "{}")).await;
    for body in ["not json", r#"{"token":"x"}"#] {
        let answer = router_of(state(sample_run(false), bases(Some(&realm), None)))
            .oneshot(internal(RUN_ID, "identity", PORTAL_TOKEN, body))
            .await
            .expect("an answer");
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST, "{body}");
        assert!(body_of(answer).await.contains("subjectToken"), "{body}");
    }
}

/// Both internal doors answer a run id that is not a name, or one the Portal does not hold, with
/// the same `404`; a Portal that cannot be read is a `503` that says so.
#[tokio::test]
async fn an_unknown_or_unshaped_run_is_a_404_and_an_unread_portal_a_503() {
    let realm = realm(|_| reply(200, "{}")).await;
    for door in ["identity", "data-credential"] {
        let unshaped = router_of(state(sample_run(false), bases(Some(&realm), None)))
            .oneshot(internal("run.1", door, PORTAL_TOKEN, &hand_over_body()))
            .await
            .expect("an answer");
        assert_eq!(unshaped.status(), StatusCode::NOT_FOUND, "{door}");
        assert!(
            body_of(unshaped).await.contains("no run holds this id"),
            "{door}"
        );

        let held_by_none = portal(404).await;
        let unknown = router_of(state(
            sample_run(false),
            bases(Some(&realm), Some(&held_by_none)),
        ))
        .oneshot(internal(OTHER_RUN, door, PORTAL_TOKEN, &hand_over_body()))
        .await
        .expect("an answer");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND, "{door}");

        let failing = portal(500).await;
        let unread = router_of(state(
            sample_run(false),
            bases(Some(&realm), Some(&failing)),
        ))
        .oneshot(internal(OTHER_RUN, door, PORTAL_TOKEN, &hand_over_body()))
        .await
        .expect("an answer");
        assert_eq!(unread.status(), StatusCode::SERVICE_UNAVAILABLE, "{door}");
        assert!(
            body_of(unread)
                .await
                .contains("the run could not be read from the Portal"),
            "{door}"
        );
    }
}

/// An exchange the realm never answers binds nothing and is the realm's `503`.
#[tokio::test]
async fn a_hand_over_whose_exchange_is_never_answered_is_the_realms_503() {
    let realm = realm(|_| None).await;
    let portal = portal(200).await;
    let answer = router_of(state(sample_run(false), bases(Some(&realm), Some(&portal))))
        .oneshot(internal(
            RUN_ID,
            "identity",
            PORTAL_TOKEN,
            &hand_over_body(),
        ))
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(body_of(answer)
        .await
        .contains("the realm could not be reached"));
}

/// A run whose grant cannot be refreshed because the realm does not answer reads nothing: the
/// data call is the proxy's `503`, never a call without the person's token.
#[tokio::test]
async fn a_data_call_whose_grant_cannot_be_refreshed_is_a_503() {
    let realm = realm(|body| {
        if body.contains("token-exchange") {
            return reply(
                200,
                &format!(
                    r#"{{"access_token":"delegated","expires_in":0,"refresh_token":"{REFRESH}"}}"#
                ),
            );
        }
        None
    })
    .await;
    let (portal, gateway) = (
        portal(200).await,
        RawUpstream::start(|_, _| reply(200, "[]")).await,
    );
    let proxy = router_of(state(
        sample_run(false),
        Bases {
            gateway: gateway.base.clone(),
            ..bases(Some(&realm), Some(&portal))
        },
    ));
    let bound = proxy
        .clone()
        .oneshot(internal(
            RUN_ID,
            "identity",
            PORTAL_TOKEN,
            &hand_over_body(),
        ))
        .await
        .expect("an answer");
    assert_eq!(bound.status(), StatusCode::NO_CONTENT);
    let read = proxy
        .oneshot(
            authed("GET", "/v1/data/ngsi-ld/v1/entities")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(read.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(body_of(read)
        .await
        .contains("could not obtain a token for this endpoint"));
    assert!(gateway.paths().is_empty(), "nothing reached the gateway");
}

/// A data credential of a run the Portal does not hold opens nothing; with the mesh check on, only
/// the functions workload may present one.
#[tokio::test]
async fn a_data_credential_needs_a_live_run_and_the_functions_workload() {
    let held_by_none = portal(404).await;
    let proxy_state = state(sample_run(false), bases(None, Some(&held_by_none)));
    let other = proxy_state.data_credentials.mint(OTHER_RUN);
    let refused = router_of(Arc::clone(&proxy_state))
        .oneshot(
            Request::builder()
                .uri("/v1/data/ngsi-ld/v1/entities")
                .header("authorization", format!("Bearer {other}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

    let mut meshed: ProxyState = (*proxy_state).clone();
    let mut config = (*meshed.config).clone();
    config.require_mesh_identity = true;
    config.functions_identity = "jc-functions.jc".to_owned();
    meshed.config = Arc::new(config);
    let meshed = Arc::new(meshed);
    let credential = meshed.data_credentials.mint(RUN_ID);
    for (identity, refused) in [
        (
            "agent-run-x.jc.serviceaccount.identity.linkerd.cluster.local",
            true,
        ),
        ("", true),
        (
            "jc-functions.jc.serviceaccount.identity.linkerd.cluster.local",
            false,
        ),
    ] {
        let answer = router_of(Arc::clone(&meshed))
            .oneshot(
                Request::builder()
                    .uri("/v1/data/ngsi-ld/v1/entities")
                    .header("authorization", format!("Bearer {credential}"))
                    .header("l5d-client-id", identity)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(
            answer.status() == StatusCode::FORBIDDEN,
            refused,
            "{identity}: {}",
            answer.status()
        );
    }
}

/// The run's doors that call the Portal with the proxy's own token answer `503` when that token
/// cannot be had: a realm that closes the connection, or one whose answer breaks off.
#[tokio::test]
async fn a_portal_door_without_the_proxys_own_token_is_a_503() {
    let closed = RawUpstream::start(|_, _| None).await;
    let cut = RawUpstream::start(|_, _| broken(200, 200, r#"{"access_token":"#)).await;
    for realm in [&closed, &cut] {
        for request in [
            authed("POST", "/v1/mcp").body(Body::from("{}")),
            authed("GET", "/v1/runs/inbox").body(Body::empty()),
            authed("GET", "/v1/diagnostics/pipeline/bikes").body(Body::empty()),
        ] {
            let answer = router_of(state(sample_run(false), bases(Some(realm), None)))
                .oneshot(request.expect("a request"))
                .await
                .expect("an answer");
            assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                body_of(answer).await,
                "the proxy could not obtain a token of its own"
            );
        }
    }
}

/// A run id that is not a name is refused with the shared sentence and logged by its length
/// alone, never the id itself.
#[tokio::test]
async fn an_unshaped_run_id_is_logged_by_its_length_alone() {
    logged();
    let answer = common::app(sample_run(false), Bases::default())
        .oneshot(
            Request::builder()
                .uri("/v1/data/ngsi-ld/v1/entities")
                .header("x-jc-run", "run.%2e%2e")
                .header("x-jc-ticket", "t")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
    let lines = logged();
    assert!(
        lines.contains("a presented run id is not shaped like a run id"),
        "{lines}"
    );
    assert!(lines.contains("length=10"), "{lines}");
    assert!(!lines.contains("run.%2e%2e"), "{lines}");
}

/// The gateway's answer: a redirect is refused and logged with where it pointed; a body that
/// breaks off is the gateway's `502`; a whole answer is relayed and is one audit line naming the
/// run, its person, the upstream and the status.
#[tokio::test]
async fn the_gateways_redirect_breakoff_and_answer_are_each_logged_and_answered() {
    logged();
    let gateway = RawUpstream::start(|path, _| {
        if path.ends_with("/moved") {
            return Some(
                b"HTTP/1.1 302 Found\r\nLocation: http://elsewhere.invalid/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            );
        }
        if path.ends_with("/cut") {
            return broken(200, 100, "[{\"id\":");
        }
        reply(200, "[]")
    })
    .await;
    // The client the proxy is built with, which follows no redirect (T-1695).
    let proxy = || {
        let mut proxy_state = (*state(
            sample_run(false),
            Bases {
                gateway: gateway.base.clone(),
                ..Bases::default()
            },
        ))
        .clone();
        proxy_state.http = agent_proxy::no_redirect_client(None);
        router_of(Arc::new(proxy_state))
    };
    let get = |path: &str| {
        authed("GET", &format!("/v1/data/{path}"))
            .body(Body::empty())
            .expect("a request")
    };

    let moved = proxy().oneshot(get("moved")).await.expect("an answer");
    assert_eq!(moved.status(), StatusCode::BAD_GATEWAY);
    let lines = logged();
    assert!(
        lines.contains("location=\"http://elsewhere.invalid/x\"")
            || lines.contains("location=http://elsewhere.invalid/x"),
        "{lines}"
    );
    assert!(lines.contains("status=302"), "{lines}");

    let cut = proxy().oneshot(get("cut")).await.expect("an answer");
    assert_eq!(cut.status(), StatusCode::BAD_GATEWAY);

    let whole = proxy()
        .oneshot(get("ngsi-ld/v1/entities"))
        .await
        .expect("an answer");
    assert_eq!(whole.status(), StatusCode::OK);
    assert_eq!(body_of(whole).await, "[]");
    let lines = logged();
    let audit = lines
        .lines()
        .rev()
        .find(|line| line.contains("proxied request") && line.contains("path=ngsi-ld/v1/entities"))
        .unwrap_or_else(|| panic!("no audit line: {lines}"));
    assert!(audit.contains(&format!("run_id={RUN_ID}")), "{audit}");
    assert!(audit.contains("user=demo.steward@hel.fi"), "{audit}");
    assert!(audit.contains("upstream=context-gateway"), "{audit}");
    assert!(audit.contains("status=200"), "{audit}");
}

/// A model call's usage that cannot be reported, because the proxy has no token of its own, is
/// still counted against the run, and the report's loss is a log line.
#[tokio::test]
async fn a_usage_report_without_the_proxys_own_token_is_logged() {
    logged();
    let silent = RawUpstream::start(|_, _| None).await;
    let model = RawUpstream::start(|_, _| {
        reply(
            200,
            r#"{"usage":{"prompt_tokens":3,"completion_tokens":4}}"#,
        )
    })
    .await;
    let proxy_state = state(
        sample_run(false),
        Bases {
            model: model.base.clone(),
            ..bases(Some(&silent), None)
        },
    );
    let answer = router_of(Arc::clone(&proxy_state))
        .oneshot(
            authed("POST", "/v1/llm/v1/messages")
                .body(Body::from(r#"{"messages":[]}"#))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(proxy_state.limits.tokens_left(RUN_ID, 1000).await, 993);
    let lines = logged_eventually("no token to report usage with").await;
    assert!(lines.contains("no token to report usage with"), "{lines}");
    assert!(
        silent
            .bodies_at("/token")
            .iter()
            .any(|b| b.contains("client_credentials")),
        "the proxy asked for a token of its own"
    );
}

/// A request body that fails while it is read is a `400`, and nothing is sent upstream.
#[tokio::test]
async fn a_body_that_fails_while_read_is_a_400() {
    let model = RawUpstream::start(|_, _| reply(200, "{}")).await;
    let chunks: Vec<Result<&'static [u8], std::io::Error>> = vec![
        Ok(b"{\"messages\":"),
        Err(std::io::Error::other("the client went away")),
    ];
    let answer = router_of(state(
        sample_run(false),
        Bases {
            model: model.base.clone(),
            ..Bases::default()
        },
    ))
    .oneshot(
        authed("POST", "/v1/llm/v1/messages")
            .body(Body::from_stream(futures::stream::iter(chunks)))
            .expect("a request"),
    )
    .await
    .expect("an answer");
    assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(answer).await.contains("failed to read body"));
    assert!(model.paths().is_empty());
}

/// An App's model call whose Portal token the realm cannot judge is the realm's `503`.
#[tokio::test]
async fn an_apps_model_call_the_realm_cannot_judge_is_a_503() {
    let silent = RawUpstream::start(|_, _| None).await;
    let answer = router_of(state(sample_run(false), bases(Some(&silent), None)))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/llm/v1/messages")
                .header("authorization", format!("Bearer {PORTAL_TOKEN}"))
                .header("x-jc-app", "helsinki/bikes")
                .body(Body::from("{}"))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(body_of(answer)
        .await
        .contains("the realm could not be reached"));
}
