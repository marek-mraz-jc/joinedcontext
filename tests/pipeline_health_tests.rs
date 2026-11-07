//! The pipelines' health route (T-5048): what a steward reads per source. Who may call it is
//! held by tests/authz_matrix.yaml.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::pipeline_log::{NewLine, Outcome};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

fn steward_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        .expect("a session cookie")
}

fn pipeline(name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Pipeline".into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some("bbsk".into()),
            ..Default::default()
        },
        spec: serde_json::json!({}),
        status: None,
    }
}

fn line(outcome: Outcome, message: &str) -> NewLine {
    NewLine {
        record_id: String::new(),
        step: None,
        outcome,
        message: message.into(),
    }
}

#[tokio::test]
async fn a_steward_reads_each_source_with_its_state_and_its_error_without_url_queries() {
    let config = Config::for_tests();
    let cookie = steward_cookie(&config);
    let mirror = Arc::new(Mirror::new());
    for name in ["podujatia-zvolen", "podujatia-detva", "podujatia-revuca"] {
        mirror.upsert(pipeline(name));
    }
    let state = AppState::new(config, None).with_mirror(mirror);
    let log = Arc::clone(&state.pipeline_log);
    log.append("bbsk", "podujatia-detva", "r1", &[line(Outcome::Sent, "")])
        .await
        .unwrap();
    let refused = "GET https://zvolen.example/rss?apikey=s3cr3t answered 403";
    log.append(
        "bbsk",
        "podujatia-zvolen",
        "r1",
        &[line(Outcome::Failed, refused)],
    )
    .await
    .unwrap();
    // Two weekly link checks of detva: one page gone both times is dead (T-5133).
    let gone = [(
        "https://detva.example/podujatia/1".to_owned(),
        joinedcontext_portal::link_check::Outcome::Failed {
            status: Some(404),
            error: None,
        },
    )];
    let now = chrono::Utc::now();
    let first = joinedcontext_portal::link_check::fold(None, &gone, 3, now);
    let links = joinedcontext_portal::link_check::fold(Some(&first), &gone, 3, now);
    state
        .link_check
        .put("bbsk", "podujatia-detva", &links)
        .await
        .unwrap();
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/bbsk/pipeline-health")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();
    let names: Vec<&str> = items
        .iter()
        .map(|i| i["pipeline"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["podujatia-detva", "podujatia-revuca", "podujatia-zvolen"]
    );
    assert_eq!(items[0]["state"], "ok");
    assert_eq!(items[0]["lastSent"], 1);
    assert_eq!(items[1]["state"], "never");
    assert_eq!(items[2]["state"], "failing");
    assert_eq!(items[2]["failedRuns"], 1);
    assert_eq!(items[2]["alert"], false);
    assert_eq!(
        items[2]["lastError"],
        "GET https://zvolen.example/rss answered 403"
    );
    assert!(!String::from_utf8_lossy(&body).contains("s3cr3t"));
    assert_eq!(items[0]["links"]["checked"], 1);
    assert_eq!(items[0]["links"]["skipped"], 3);
    assert_eq!(items[0]["links"]["deadCount"], 1);
    assert_eq!(
        items[0]["links"]["dead"][0],
        serde_json::json!({"url": "https://detva.example/podujatia/1", "status": 404})
    );
    assert!(items[0]["links"].get("failing").is_none());
    assert!(items[1].get("links").is_none());
}
