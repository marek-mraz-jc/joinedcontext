//! `GET /api/v1/organization/datamodels` (T-2701, DM-63, ADR-N-033): the one list behind the data
//! model and type pickers. It is the endpoint listing's read rule applied to `DataModel`, project by
//! project and space by space, with the Smart Data Models entries a search matches beside it.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const HELSINKI: &str = "helsinki";
const ESPOO: &str = "espoo";
const READER: &str = "mira.models@hel.fi";
const SPACE_READER: &str = "sami.air@hel.fi";
const STRANGER: &str = "nobody@hel.fi";

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

fn model(
    project: &str,
    name: &str,
    space: &str,
    lifecycle: &str,
    classes: &[&str],
) -> ResourceEnvelope {
    envelope(
        "DataModel",
        name,
        project,
        json!({
            "contextSpaceRef": space,
            "linkml": format!("./{name}.linkml.yaml"),
            "version": "1.2.0",
            "lifecycle": lifecycle,
            "classes": classes,
        }),
    )
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
}

/// Two projects, two spaces in helsinki, a retired model, and three callers: a reader of helsinki's
/// models, a reader bound to helsinki's `air` space alone, and a person no binding names.
fn world(config: Config) -> AppState {
    let state = AppState::new(config, None);
    for (project, space) in [(HELSINKI, "air"), (HELSINKI, "mobility"), (ESPOO, "ilma")] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, project, json!({})));
    }
    state.mirror.upsert(model(
        HELSINKI,
        "air-quality",
        "air",
        "published",
        &["AirQualityObserved"],
    ));
    state.mirror.upsert(model(
        HELSINKI,
        "bikes",
        "mobility",
        "draft",
        &["BikeHireDockingStation"],
    ));
    state.mirror.upsert(model(
        HELSINKI,
        "old-bikes",
        "mobility",
        "retired",
        &["BikeStation"],
    ));
    state.mirror.upsert(model(
        ESPOO,
        "espoo-air",
        "ilma",
        "published",
        &["AirQualityObserved"],
    ));
    state.mirror.upsert(envelope(
        "Role",
        "model-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["DataModel"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "model-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": READER }], "role": "model-reader",
                "scope": { "project": HELSINKI } }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-model-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": SPACE_READER }], "role": "model-reader",
                "scope": { "contextSpace": "air" } }),
    ));
    state
}

async fn get(state: &AppState, email: Option<&str>, uri: &str) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(uri);
    if let Some(email) = email {
        request = request.header(header::COOKIE, cookie(&state.config, email));
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("a request"))
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

fn names(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            format!(
                "{}/{}",
                item["project"].as_str().unwrap_or(""),
                item["name"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// DM-63, R20: the list is filtered by read permission, project by project and space by space; a
/// retired version is offered to nobody; a stranger gets an empty list and never a refusal.
#[tokio::test]
async fn the_list_holds_only_the_models_the_caller_may_read() {
    let state = world(Config::for_tests());

    let (status, body) = get(&state, Some(READER), "/api/v1/organization/datamodels").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        names(&body),
        ["helsinki/air-quality", "helsinki/bikes"],
        "{body}"
    );
    assert_eq!(
        body["items"][0],
        json!({ "name": "air-quality", "level": "project", "project": HELSINKI, "space": "air",
                "version": "1.2.0", "lifecycle": "published", "classes": ["AirQualityObserved"] }),
    );
    assert!(
        !body.to_string().contains(ESPOO),
        "another project's model leaked: {body}"
    );
    assert!(
        !body.to_string().contains("old-bikes"),
        "a retired model is offered: {body}"
    );

    let (status, body) = get(
        &state,
        Some(SPACE_READER),
        "/api/v1/organization/datamodels",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        names(&body),
        ["helsinki/air-quality"],
        "a space-scoped reader saw another space: {body}"
    );

    let (status, body) = get(&state, Some(STRANGER), "/api/v1/organization/datamodels").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"], json!([]), "{body}");

    let (status, _) = get(&state, None, "/api/v1/organization/datamodels").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The search matches a class, a space or a name case-insensitively, and never widens the list.
#[tokio::test]
async fn search_matches_name_space_and_class_and_is_bounded() {
    let state = world(Config::for_tests());

    let (_, body) = get(
        &state,
        Some(READER),
        "/api/v1/organization/datamodels?search=bikehire",
    )
    .await;
    assert_eq!(names(&body), ["helsinki/bikes"], "{body}");
    let (_, body) = get(
        &state,
        Some(READER),
        "/api/v1/organization/datamodels?search=AIR",
    )
    .await;
    assert_eq!(names(&body), ["helsinki/air-quality"], "{body}");
    let (_, body) = get(
        &state,
        Some(SPACE_READER),
        "/api/v1/organization/datamodels?search=bikes",
    )
    .await;
    assert_eq!(
        body["items"],
        json!([]),
        "search widened a space-scoped list: {body}"
    );
    let (_, body) = get(
        &state,
        Some(READER),
        "/api/v1/organization/datamodels?search=%20%20",
    )
    .await;
    assert_eq!(names(&body).len(), 2, "a blank search is no search: {body}");

    let long = "a".repeat(101);
    let (status, body) = get(
        &state,
        Some(READER),
        &format!("/api/v1/organization/datamodels?search={long}"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("100 characters"), "{body}");
}

/// A search of two characters or more also lists the catalogue entries it matches; one character
/// asks nothing of Model Tools.
#[tokio::test]
async fn a_search_lists_matching_smart_data_models() {
    let tools = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subjects": [
                { "name": "dataModel.Environment", "models": [
                    { "id": "dataModel.Environment/AirQualityObserved", "name": "AirQualityObserved",
                      "description": "An observation of air quality", "attributes": ["pm10"] },
                    { "id": "dataModel.Environment/NoiseLevelObserved", "name": "NoiseLevelObserved" }
                ]},
                { "name": "dataModel.Transportation", "models": [
                    { "id": "dataModel.Transportation/BikeHireDockingStation", "name": "BikeHireDockingStation" }
                ]}
            ]
        })))
        .expect(1)
        .mount(&tools)
        .await;
    let state = world(Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    });

    let (_, body) = get(
        &state,
        Some(READER),
        "/api/v1/organization/datamodels?search=a",
    )
    .await;
    assert_eq!(
        body["smartDataModels"],
        json!([]),
        "one character reached the catalogue: {body}"
    );

    let (status, body) = get(
        &state,
        Some(READER),
        "/api/v1/organization/datamodels?search=air",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["smartDataModels"],
        json!([{ "id": "dataModel.Environment/AirQualityObserved", "name": "AirQualityObserved",
                 "subject": "dataModel.Environment", "description": "An observation of air quality" }]),
    );
    assert!(body.get("catalogueUnavailable").is_none(), "{body}");
}

/// A catalogue Model Tools cannot answer leaves the organization's own models listed and says why.
#[tokio::test]
async fn a_missing_catalogue_still_lists_the_organizations_models() {
    let state = world(Config::for_tests());
    let (status, body) = get(
        &state,
        Some(READER),
        "/api/v1/organization/datamodels?search=air",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(names(&body), ["helsinki/air-quality"], "{body}");
    assert_eq!(body["smartDataModels"], json!([]), "{body}");
    assert!(
        body["catalogueUnavailable"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "{body}"
    );
}

/// DM-75, DM-79: an organization model is listed first, with `level: organization` and no space,
/// to every person holding any binding in the organization; a project model no space owns is its
/// project's readers' alone, not a space-scoped reader's; a stranger sees neither.
#[tokio::test]
async fn organization_models_are_every_members_and_carry_their_level() {
    let state = world(Config::for_tests());
    let mut shared = model(ORG_NAMESPACE, "stations", "", "published", &["Station"]);
    shared
        .spec
        .as_object_mut()
        .expect("spec")
        .remove("contextSpaceRef");
    state.mirror.upsert(shared);
    let mut own = model(HELSINKI, "helsinki-shared", "", "published", &["Kiosk"]);
    own.spec
        .as_object_mut()
        .expect("spec")
        .remove("contextSpaceRef");
    state.mirror.upsert(own);

    let (status, body) = get(&state, Some(READER), "/api/v1/organization/datamodels").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        names(&body),
        [
            "org/stations",
            "helsinki/helsinki-shared",
            "helsinki/air-quality",
            "helsinki/bikes"
        ],
        "{body}"
    );
    assert_eq!(
        body["items"][0],
        json!({ "name": "stations", "level": "organization", "project": ORG_NAMESPACE,
                "version": "1.2.0", "lifecycle": "published", "classes": ["Station"] }),
    );
    assert_eq!(body["items"][1]["level"], "project");
    assert!(body["items"][1].get("space").is_none(), "{body}");

    let (_, body) = get(
        &state,
        Some(SPACE_READER),
        "/api/v1/organization/datamodels",
    )
    .await;
    assert_eq!(
        names(&body),
        ["org/stations", "helsinki/air-quality"],
        "a space-scoped reader reads the organization's models and not the project's own: {body}"
    );

    let (_, body) = get(&state, Some(STRANGER), "/api/v1/organization/datamodels").await;
    assert_eq!(body["items"], json!([]), "{body}");
}
