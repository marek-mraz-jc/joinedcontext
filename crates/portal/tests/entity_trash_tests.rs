//! API/01 §31, T-3107: what a person deletes from a data view they keep a copy of for 30 days,
//! their own and nobody else's; the copy must be an NGSI-LD entity of a URN of its type, and a
//! space they may not read is a 404 like every space route.

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

const PROJECT: &str = "helsinki";
const READER: &str = "sami.air@hel.fi";
const OTHER: &str = "anna.air@hel.fi";
const STRANGER: &str = "nobody@hel.fi";
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

/// Two spaces of helsinki; two readers bound to `air` alone.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
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
        json!({ "subjects": [{ "user": READER }, { "user": OTHER }], "role": "space-reader",
                "scope": { "contextSpace": "air" } }),
    ));
    state
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
        .header(header::COOKIE, cookie(&state.config, email))
        .header(CSRF_HEADER, CSRF)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("a request");
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

const TRASH: &str = "/api/v1/projects/helsinki/spaces/air/trash";

fn station(n: u32) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:AirQualityObserved:hel.fi:air:{n}"),
        "type": "AirQualityObserved",
        "pm10": { "type": "Property", "value": 12 }
    })
}

#[tokio::test]
async fn a_reader_keeps_a_copy_lists_their_own_newest_first_and_forgets_it() {
    let state = world();
    let (status, kept) = call(
        &state,
        READER,
        "POST",
        TRASH,
        Some(json!({ "entity": station(1) })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{kept}");
    assert_eq!(kept["urn"], "urn:ngsi-ld:AirQualityObserved:hel.fi:air:1");
    assert_eq!(kept["type"], "AirQualityObserved");
    assert_eq!(kept["entity"], station(1));
    assert!(kept["expiresAt"]
        .as_str()
        .is_some_and(|at| at > kept["deletedAt"].as_str().unwrap_or_default()));
    call(
        &state,
        READER,
        "POST",
        TRASH,
        Some(json!({ "entity": station(2) })),
    )
    .await;

    let (status, list) = call(&state, READER, "GET", TRASH, None).await;
    assert_eq!(status, StatusCode::OK);
    let urns: Vec<&str> = list
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|i| i["urn"].as_str())
        .collect();
    assert_eq!(
        urns,
        [
            "urn:ngsi-ld:AirQualityObserved:hel.fi:air:2",
            "urn:ngsi-ld:AirQualityObserved:hel.fi:air:1"
        ]
    );

    // Another reader of the same space sees none of it and cannot forget it.
    let (_, theirs) = call(&state, OTHER, "GET", TRASH, None).await;
    assert_eq!(theirs, json!([]));
    let id = kept["id"].as_i64().expect("an id");
    let (status, _) = call(&state, OTHER, "DELETE", &format!("{TRASH}/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(&state, READER, "DELETE", &format!("{TRASH}/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&state, READER, "DELETE", &format!("{TRASH}/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, list) = call(&state, READER, "GET", TRASH, None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn a_copy_is_an_ngsi_ld_entity_of_its_own_type_and_no_larger_than_the_limit() {
    let state = world();
    for (entity, said) in [
        (
            json!({ "id": "urn:ngsi-ld:Device:hel.fi:air:1", "type": "AirQualityObserved" }),
            "URN of its type",
        ),
        (
            json!({ "id": "station-1", "type": "AirQualityObserved" }),
            "URN of its type",
        ),
        (
            json!({ "id": "urn:ngsi-ld:1x:a", "type": "1x" }),
            "type name",
        ),
        (json!({ "type": "AirQualityObserved" }), "URN of its type"),
        (
            json!({ "id": "urn:ngsi-ld:AirQualityObserved:a b", "type": "AirQualityObserved" }),
            "URN of its type",
        ),
    ] {
        let (status, body) = call(
            &state,
            READER,
            "POST",
            TRASH,
            Some(json!({ "entity": entity })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{entity}");
        assert!(
            body["detail"].as_str().unwrap_or_default().contains(said),
            "{body}"
        );
    }
    let mut big = station(3);
    big["note"] = json!({ "type": "Property", "value": "x".repeat(300 * 1024) });
    let (status, body) = call(
        &state,
        READER,
        "POST",
        TRASH,
        Some(json!({ "entity": big })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("at most"),
        "{body}"
    );
    let (status, _) = call(
        &state,
        READER,
        "POST",
        TRASH,
        Some(json!({ "entity": station(1), "extra": 1 })),
    )
    .await;
    assert!(status.is_client_error(), "an unknown key is refused");
}

#[tokio::test]
async fn a_space_the_caller_may_not_read_is_a_404_on_every_trash_route() {
    let state = world();
    for (who, uri) in [
        (STRANGER, TRASH.to_owned()),
        (
            READER,
            "/api/v1/projects/helsinki/spaces/mobility/trash".to_owned(),
        ),
        (
            READER,
            "/api/v1/projects/helsinki/spaces/nowhere/trash".to_owned(),
        ),
    ] {
        assert_eq!(
            call(&state, who, "GET", &uri, None).await.0,
            StatusCode::NOT_FOUND,
            "{who} {uri}"
        );
        assert_eq!(
            call(
                &state,
                who,
                "POST",
                &uri,
                Some(json!({ "entity": station(1) }))
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
            "{who} {uri}"
        );
        assert_eq!(
            call(&state, who, "DELETE", &format!("{uri}/1"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
}
