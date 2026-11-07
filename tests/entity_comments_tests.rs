//! API/01 §35, T-3106: comments on a space's entities with @mentions. A reader of the space
//! comments and removes only their own; a mention notifies a person who may read the space and
//! nobody else; each person reads and marks only their own notifications; a space the caller may
//! not read is a 404 like every space route. T-3284: listing or adding the comments of an entity
//! reads that entity on the gateway's space surface with the caller's own token first, so an
//! entity their Policy hides is the same 404 as one that is not there.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "helsinki";
const READER: &str = "sami.air@hel.fi";
const OTHER: &str = "anna.air@hel.fi";
const STRANGER: &str = "nobody@hel.fi";
/// Reads `air` through a Group, not by name.
const GROUPED: &str = "pia.air@hel.fi";
const CSRF: &str = "test-csrf-token-12345";

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

fn cookie(config: &Config, email: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let username = email.split('@').next().unwrap_or(email).to_owned();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username,
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
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
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect::<Vec<_>>()
        .join("; ")
        + &format!("; {CSRF_COOKIE}={CSRF}")
}

/// The Portal with its gateway: a stub that lets every caller read the entities of `air` except
/// [`HIDDEN`], which it refuses as a Policy would. Kept alive beside the state.
struct World {
    state: AppState,
    gateway: MockServer,
}

impl std::ops::Deref for World {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.state
    }
}

/// Two spaces of helsinki; two readers bound to `air` alone.
async fn world() -> World {
    let gateway = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/air/ngsi-ld/v1/entities/{HIDDEN}")))
        .respond_with(ResponseTemplate::new(403))
        .with_priority(1)
        .mount(&gateway)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/air/ngsi-ld/v1/entities/{URN}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": URN, "type": "AirQualityObserved" })),
        )
        .mount(&gateway)
        .await;
    let gateway_url = gateway.uri();
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_PORTAL_GATEWAY_URL" => Some(gateway_url.as_str()),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let state = AppState::new(config, None);
    for space in ["air", "mobility"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": READER }, { "user": OTHER }, { "group": "air-team" }],
                "role": "space-reader", "scope": { "contextSpace": "air" } }),
    ));
    state.mirror.upsert(envelope(
        "Group",
        "air-team",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": GROUPED }] }),
    ));
    World { state, gateway }
}

/// The person's own access token, as the edge or a script presents it: `who@hel.fi`.
fn token(email: &str) -> String {
    let who = email.split('@').next().unwrap_or(email);
    common::REALM.person_token_of("portal-api", "portal-api", who, &[])
}

async fn call(
    state: &AppState,
    email: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token(email)))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("a request");
    answer(state, request).await
}

/// The `sub` of a JWT, read without checking it: the test only asks whose token went out.
fn subject_of(jwt: &str) -> String {
    use base64::Engine as _;
    let payload = jwt.split('.').nth(1).expect("a JWT");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("base64url");
    let claims: Value = serde_json::from_slice(&bytes).expect("claims");
    claims["sub"].as_str().unwrap_or_default().to_owned()
}

/// The same person with the Portal's own cookie session, which holds no access token.
async fn call_with_cookie(state: &AppState, email: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, email))
        .header(CSRF_HEADER, CSRF)
        .body(Body::empty())
        .expect("a request");
    answer(state, request).await
}

async fn answer(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const COMMENTS: &str = "/api/v1/projects/helsinki/spaces/air/comments";
const URN: &str = "urn:ngsi-ld:AirQualityObserved:hel.fi:air:1";
/// An entity of `air` the gateway's Policy hides from every caller here.
const HIDDEN: &str = "urn:ngsi-ld:Salary:hel.fi:air:7";

fn listed(urn: &str) -> String {
    format!("{COMMENTS}?urn={urn}")
}

#[tokio::test]
async fn a_reader_comments_mentions_readers_of_the_space_and_only_they_are_notified() {
    let world = world().await;
    let state = &world.state;
    let text = format!("@{OTHER} and @{GROUPED}: pm10 jumped. cc @{STRANGER} @{READER}");
    let (status, made) = call(
        state,
        READER,
        "POST",
        COMMENTS,
        Some(json!({ "urn": URN, "text": text })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["urn"], URN);
    assert_eq!(made["author"], READER);
    assert_eq!(made["mine"], true);
    assert_eq!(made["mentions"], json!([OTHER, GROUPED]));
    // A stranger is kept in the text and notified to nobody; the author never notifies themself.
    assert_eq!(made["unknownMentions"], json!([STRANGER]));

    for person in [OTHER, GROUPED] {
        let (status, inbox) = call(state, person, "GET", "/api/v1/notifications", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(inbox["unread"], 1, "{person}: {inbox}");
        assert_eq!(inbox["items"][0]["urn"], URN);
        assert_eq!(inbox["items"][0]["space"], "air");
        assert_eq!(inbox["items"][0]["author"], READER);
        assert!(inbox["items"][0]["excerpt"]
            .as_str()
            .is_some_and(|e| e.contains("pm10 jumped")));
    }
    for person in [STRANGER, READER] {
        let (_, inbox) = call(state, person, "GET", "/api/v1/notifications", None).await;
        assert_eq!(inbox["items"], json!([]), "{person}");
    }

    let (status, list) = call(state, OTHER, "GET", &listed(URN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["mine"], false);
}

#[tokio::test]
async fn a_notification_is_read_by_its_recipient_alone() {
    let world = world().await;
    let state = &world.state;
    call(
        state,
        READER,
        "POST",
        COMMENTS,
        Some(json!({ "urn": URN, "text": format!("@{OTHER} look") })),
    )
    .await;
    let (_, inbox) = call(state, OTHER, "GET", "/api/v1/notifications", None).await;
    let id = inbox["items"][0]["id"].as_i64().expect("an id");
    let read = format!("/api/v1/notifications/{id}/read");
    let (status, _) = call(state, READER, "POST", &read, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another person's notification"
    );
    let (status, _) = call(state, OTHER, "POST", &read, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, inbox) = call(state, OTHER, "GET", "/api/v1/notifications", None).await;
    assert_eq!(inbox["unread"], 0);
    assert_eq!(inbox["items"][0]["read"], true);
}

#[tokio::test]
async fn only_the_author_removes_a_comment_and_its_notifications_go_with_it() {
    let world = world().await;
    let state = &world.state;
    let (_, made) = call(
        state,
        READER,
        "POST",
        COMMENTS,
        Some(json!({ "urn": URN, "text": format!("@{OTHER} hi") })),
    )
    .await;
    let id = made["id"].as_i64().expect("an id");
    let one = format!("{COMMENTS}/{id}");
    let (status, _) = call(state, OTHER, "DELETE", &one, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "nobody removes another's comment"
    );
    let (status, _) = call(state, READER, "DELETE", &one, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = call(state, OTHER, "GET", &listed(URN), None).await;
    assert_eq!(list, json!([]));
    let (_, inbox) = call(state, OTHER, "GET", "/api/v1/notifications", None).await;
    assert_eq!(inbox["items"], json!([]));
}

#[tokio::test]
async fn a_comment_needs_a_urn_and_a_text_of_its_size_and_a_readable_space() {
    let world = world().await;
    let state = &world.state;
    for body in [
        json!({ "urn": "station 7", "text": "x" }),
        json!({ "urn": URN, "text": "   " }),
        json!({ "urn": URN, "text": "x".repeat(4001) }),
    ] {
        let (status, problem) = call(state, READER, "POST", COMMENTS, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {problem}");
    }
    let (status, _) = call(
        state,
        READER,
        "POST",
        COMMENTS,
        Some(json!({ "urn": URN, "text": "x", "author": "me" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown keys are refused"
    );
    let (status, _) = call(state, READER, "GET", &format!("{COMMENTS}?urn=bad"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A space the caller may not read, and one they read but that is not this one, are 404.
    let (status, _) = call(state, STRANGER, "GET", &listed(URN), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        state,
        READER,
        "POST",
        "/api/v1/projects/helsinki/spaces/mobility/comments",
        Some(json!({ "urn": URN, "text": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// T-3284: an entity the caller's Policy hides has no comments for them, read or written, and
/// the gateway was asked with their own token; a cookie session, which carries none, is told why.
#[tokio::test]
async fn the_comments_of_an_entity_follow_the_callers_read_of_that_entity() {
    let world = world().await;
    let state = &world.state;
    let (status, problem) = call(state, READER, "GET", &listed(HIDDEN), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    let (status, _) = call(
        state,
        READER,
        "POST",
        COMMENTS,
        Some(json!({ "urn": HIDDEN, "text": "raised in October" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(state, READER, "GET", &listed(URN), None).await;
    assert_eq!(status, StatusCode::OK);
    let asked = world.gateway.received_requests().await.unwrap_or_default();
    assert!(!asked.is_empty());
    for request in &asked {
        let bearer = request
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .expect("a bearer");
        assert_eq!(
            subject_of(bearer),
            "sub-sami.air",
            "the caller's own token, never the Portal's"
        );
    }

    let (status, problem) = call_with_cookie(state, READER, &listed(URN)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("your own access token"),
        "{problem}"
    );
}
