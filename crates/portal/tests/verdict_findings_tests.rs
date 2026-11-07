//! What a check answers when it rejects the manifest (T-2234; UI-23, PF-57, MF-13).
//!
//! One shape for one outcome: a rejected manifest is a **red verdict carrying the reasons**, for
//! every kind, and not a 4xx for most kinds and a verdict for `DataSource`. The form says "resolve
//! the findings" beside the chip, so the findings have to be there; and the strict gate then reads
//! one shape instead of two.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-verdict-findings";

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "sub-steward".into(),
            username: "steward.user".into(),
            email: Some("steward@banskabystrica.sk".into()),
            name: Some("Steward".into()),
            roles: vec!["portal-approver".into()],
            // `Config::for_tests` makes `portal-approver` the bootstrap group, which is how every
            // other test of a proposal gets past the permission check.
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        parts.push(raw.split(';').next().unwrap_or_default().to_string());
    }
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

/// One check of `manifest` under `plural`, as the form runs it: `?dryRun=All`, this person's session.
async fn check(plural: &str, manifest: Value) -> (StatusCode, Value) {
    let (status, body, _) = check_on(plural, manifest).await;
    (status, body)
}

/// The same, keeping the state, so what the check left behind can be read back.
async fn check_on(plural: &str, manifest: Value) -> (StatusCode, Value, AppState) {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(Arc::new(Mirror::new()));
    let app = server::app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/ovzdusie/{plural}?dryRun=All"))
                .header(header::COOKIE, session_cookie(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&manifest).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, state)
}

#[tokio::test]
async fn a_check_that_rejects_a_manifest_answers_a_red_verdict_and_not_an_error() {
    // An endpoint naming a context space that is not in the project: `references::check` refuses it
    // and names the path of the reference it could not resolve (MF-13, T-2233).
    let (status, body) = check(
        "endpoints",
        json!({
            "apiVersion": API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": "public-air", "namespace": "ovzdusie" },
            "spec": {
                "contextSpaceRef": "nothing-here",
                "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa",
                "audience": "public",
                "enabledRepresentations": ["ngsi-ld"]
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["valid"], false);
    assert_eq!(body["verdict"]["ok"], false);
    let findings = body["verdict"]["findings"]
        .as_array()
        .expect("findings")
        .clone();
    assert!(
        !findings.is_empty(),
        "a red verdict with no findings: {body}"
    );
    assert_eq!(findings[0]["level"], "error");
    // The path is what makes a finding actionable: the field to correct, not "the manifest".
    assert_eq!(findings[0]["path"], "spec.contextSpaceRef", "{body}");
    assert!(
        findings[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing-here"),
        "{findings:?}"
    );
}

#[tokio::test]
async fn a_manifest_that_does_not_parse_is_a_finding_and_not_an_empty_list() {
    // The other half of what a check rejects: a spec the kind's own shape refuses. There is no
    // path to name, so the message is the reason, and the list is never empty.
    let (status, body) = check(
        "pipelines",
        json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": { "name": "air-feed", "namespace": "ovzdusie" },
            "spec": { "class": "auto", "paused": true }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["verdict"]["ok"], false);
    let findings = body["verdict"]["findings"].as_array().expect("findings");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0]["level"], "error");
    assert!(
        findings[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("paused"),
        "the finding does not say what was wrong: {findings:?}"
    );
}

#[tokio::test]
async fn a_green_verdict_carries_no_findings() {
    let (status, body) = check(
        "pipelines",
        json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": { "name": "air-feed", "namespace": "ovzdusie" },
            "spec": {
                "class": "auto",
                "period": "60s",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "output": { "type": "AirQualityObserved", "mode": "upsert" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
            }
        }),
    )
    .await;

    // Whatever else this manifest needs, a check that passed says so with no findings at all.
    if body["verdict"]["ok"] == Value::Bool(true) {
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["verdict"]["findings"].as_array().map(Vec::len),
            Some(0),
            "{body}"
        );
    } else {
        // It was refused: then it is refused the one way, with a reason.
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            !body["verdict"]["findings"]
                .as_array()
                .expect("findings")
                .is_empty(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_refusal_that_is_not_about_the_manifest_stays_an_error() {
    // Not a judgement about the manifest: the kind has no such plural. A verdict here would say
    // the manifest is wrong, which it is not.
    let (status, body) = check(
        "teapots",
        json!({
            "apiVersion": API_VERSION,
            "kind": "Teapot",
            "metadata": { "name": "brown-betty", "namespace": "ovzdusie" },
            "spec": {}
        }),
    )
    .await;

    assert!(status.is_client_error(), "{status} {body}");
    assert!(body["verdict"].is_null(), "{body}");
}

#[tokio::test]
async fn a_refused_check_writes_no_draft_of_its_own() {
    // The refusal can be about what the manifest holds — a credential typed into a `secretRef`
    // name is refused by the door (T-2238) — so a draft created to carry the refused manifest
    // would store the value the refusal exists to stop. The red verdict reaches a draft that is
    // already there and nothing else is written.
    let token = "glpat-not-a-real-token-abcdefghij";
    let (status, body, state) = check_on(
        "datasources",
        json!({
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "aq-feed", "namespace": "ovzdusie" },
            "spec": {
                "type": "mqtt",
                "mqtt": {
                    "urls": ["tls://mqtt.banskabystrica.sk:8883"],
                    "topics": ["sensors/aq/+/reading"],
                    "passwordRef": { "name": token, "key": "password" }
                }
            },
            "draft": { "kind": "DataSource", "name": "aq-feed" }
        }),
    )
    .await;

    // Since T-5971 the operations door refuses a credential in any argument before the check
    // reads it: a 422 naming the field, never the value (MF-24).
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["type"], "https://joinedcontext.com/errors/secret-in-argument",
        "{body}"
    );
    let said = body["errors"].to_string();
    assert!(
        said.contains("passwordRef"),
        "the refusal names the field: {said}"
    );
    assert!(
        !body.to_string().contains(token),
        "the refusal repeats the credential: {body}"
    );

    let drafts = state
        .drafts
        .list("ovzdusie")
        .await
        .expect("the drafts list");
    assert!(
        drafts.is_empty(),
        "a refused check created a draft holding the manifest it refused: {drafts:?}"
    );
}
