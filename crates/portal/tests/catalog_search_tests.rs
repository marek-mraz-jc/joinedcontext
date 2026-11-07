//! The catalog search, `jc_catalog_search` (T-0578, AG-58, API/01 §18): what matches, what the caller
//! may open, how fresh it is, and what a restricted item does not say.
use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF_TOKEN: &str = "test-csrf-catalog";

const RUNNER_BODY: &str = concat!(
    "# TYPE input_received counter\n",
    "input_received{label=\"http\",stream=\"helsinki.hsl-bikes\"} 1200\n",
    "output_sent{label=\"gw\",stream=\"helsinki.hsl-bikes\"} 1190\n",
    "output_error{label=\"gw\",stream=\"helsinki.hsl-bikes\"} 10\n",
);

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@hel.fi".into()),
            name: Some("Demo Steward".into()),
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
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie header")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn manifest(
    kind: &str,
    project: &str,
    name: &str,
    title: Option<&str>,
    spec: Value,
) -> ResourceEnvelope {
    let mut meta = serde_json::to_value(ObjectMeta::new(name, project)).expect("meta");
    if let Some(title) = title {
        meta["title"] = json!({ "en": title });
        meta["description"] =
            json!({ "en": format!("{title}: stations, positions and availability") });
    }
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: serde_json::from_value(meta).expect("meta back"),
        spec,
        status: None,
    }
}

/// Helsinki: a space with three endpoints (public, organization, a project-list naming another
/// project), a model of the space, a pipeline feeding the bikes endpoint, and a second project
/// whose bikes must never appear.
fn helsinki() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(manifest(
        "ContextSpace",
        "helsinki",
        "helsinki",
        Some("Helsinki city context"),
        json!({ "dataModelRef": "helsinki-model" }),
    ));
    mirror.upsert(manifest(
        "Endpoint",
        "helsinki",
        "helsinki-bikes",
        Some("Helsinki city bikes"),
        json!({ "contextSpaceRef": "helsinki", "slug": "bikesslug00000000000000000000000", "audience": "public", "enabledRepresentations": ["ngsi-ld"] }),
    ));
    mirror.upsert(manifest(
        "Endpoint",
        "helsinki",
        "helsinki-transport",
        Some("Helsinki vehicle positions"),
        json!({ "contextSpaceRef": "helsinki", "slug": "transportslug0000000000000000000", "audience": "organization", "enabledRepresentations": ["ngsi-ld"] }),
    ));
    mirror.upsert(manifest(
        "Endpoint",
        "helsinki",
        "helsinki-partners",
        Some("Helsinki bike partner feed"),
        json!({ "contextSpaceRef": "helsinki", "slug": "partnerslug000000000000000000000", "audience": "project-list", "allowedProjects": ["espoo"], "enabledRepresentations": ["ngsi-ld"] }),
    ));
    mirror.upsert(manifest(
        "DataModel",
        "helsinki",
        "helsinki-model",
        None,
        json!({ "contextSpaceRef": "helsinki", "classes": ["BikeHireDockingStation", "Vehicle"], "linkml": "id: x" }),
    ));
    mirror.upsert(manifest(
        "Pipeline",
        "helsinki",
        "hsl-bikes",
        None,
        json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-bikes" }),
    ));
    mirror.upsert(manifest(
        "Endpoint",
        "espoo",
        "espoo-bikes",
        Some("Espoo city bikes"),
        json!({ "contextSpaceRef": "espoo", "slug": "espooslug00000000000000000000000", "audience": "public" }),
    ));
    mirror
}

/// Runs `jc_catalog_search` in helsinki, the one door the search has (T-4453).
async fn search(config: Config, cookie: Option<&str>, input: Value) -> (StatusCode, Value) {
    let app = server::app(AppState::new(config, None).with_mirror(helsinki()));
    let csrf = format!("{CSRF_COOKIE}={CSRF_TOKEN}");
    let cookies = cookie.map_or(csrf.clone(), |cookie| format!("{cookie}; {csrf}"));
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/projects/helsinki/ops/jc_catalog_search")
        .header(header::COOKIE, cookies)
        .header(CSRF_HEADER, CSRF_TOKEN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(input.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn names(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|i| {
                    format!(
                        "{}/{}",
                        i["kind"].as_str().unwrap_or(""),
                        i["name"].as_str().unwrap_or("")
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn anonymous_search_is_401_and_an_empty_question_is_refused() {
    let config = Config::for_tests();
    let (status, _) = search(config.clone(), None, json!({ "q": "bike" })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let cookie = session_cookie(&config);
    // The schema's pattern refuses a blank q before the search runs (T-6274).
    let (status, _) = search(config.clone(), Some(&cookie), json!({ "q": " " })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = search(
        config,
        Some(&cookie),
        json!({ "q": "bike", "scope": "Pipeline" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown scope is refused, not ignored"
    );
}

/// T-6274: the schema a model reads says q is required and not blank, so a call it admits is
/// never refused for a missing q, and `{scope}` alone is refused naming q.
#[tokio::test]
async fn the_schema_requires_q_and_a_scope_alone_is_refused_naming_it() {
    let op = joinedcontext_portal::ops::registry()
        .iter()
        .find(|op| op.name == "jc_catalog_search")
        .expect("jc_catalog_search is registered");
    let schema = (op.input)();
    assert_eq!(schema["required"], json!(["q"]));
    let blank = regex::Regex::new(schema["properties"]["q"]["pattern"].as_str().unwrap()).unwrap();
    assert!(!blank.is_match("") && !blank.is_match("  ") && blank.is_match("bike"));

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let (status, body) = search(config, Some(&cookie), json!({ "scope": "Endpoint" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["errors"][0]["message"],
        json!("\"q\" is a required property"),
        "{body}"
    );
}

#[tokio::test]
async fn the_words_of_a_question_find_endpoints_spaces_and_models_of_this_project_only() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let (status, body) = search(
        config,
        Some(&cookie),
        json!({ "q": "vehicle positions or bike availability" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let found = names(&body);
    assert!(
        found.contains(&"Endpoint/helsinki-bikes".to_owned()),
        "{found:?}"
    );
    assert!(
        found.contains(&"Endpoint/helsinki-transport".to_owned()),
        "{found:?}"
    );
    assert!(
        found.contains(&"DataModel/helsinki-model".to_owned()),
        "a model matches by its classes: {found:?}"
    );
    assert!(
        found.contains(&"ContextSpace/helsinki".to_owned()),
        "the space comes through its endpoints: {found:?}"
    );
    assert!(
        !found.iter().any(|n| n.contains("espoo")),
        "another project's manifests never appear: {found:?}"
    );

    let bikes = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "helsinki-bikes")
        .expect("the bikes endpoint");
    assert_eq!(bikes["access"]["verdict"], "allowed");
    assert_eq!(bikes["access"]["reason"], "audience public");
    assert_eq!(bikes["owner"], "helsinki");
    assert_eq!(bikes["space"], "helsinki");
    assert_eq!(bikes["endpointSlug"], "bikesslug00000000000000000000000");
    assert!(
        bikes["matchReason"]
            .as_array()
            .unwrap()
            .contains(&json!("description")),
        "{bikes}"
    );
    assert!(
        bikes["freshness"].is_null(),
        "no runner is configured, so freshness is null and not a guess"
    );
    let first = &body["items"][0];
    assert_eq!(
        first["kind"], "Endpoint",
        "the best match is an endpoint the person can open: {first}"
    );
}

#[tokio::test]
async fn a_restricted_endpoint_shows_its_name_and_kind_and_nothing_else() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let (status, body) = search(
        config,
        Some(&cookie),
        json!({ "q": "partner feed", "scope": "Endpoint" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(names(&body), vec!["Endpoint/helsinki-partners"]);
    let item = &body["items"][0];
    assert_eq!(item["access"]["verdict"], "restricted");
    assert!(item["access"]["reason"]
        .as_str()
        .unwrap()
        .contains("does not name helsinki"));
    assert!(
        item.get("title").is_none(),
        "a restricted item discloses no title: {item}"
    );
    assert!(item.get("endpointSlug").is_none(), "nor its slug: {item}");
    assert!(item["freshness"].is_null());
}

#[tokio::test]
async fn freshness_is_the_runner_counters_of_the_pipeline_feeding_the_endpoint() {
    let runner = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/helsinki/metrics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RUNNER_BODY))
        .mount(&runner)
        .await;
    let mut config = Config::for_tests();
    config.pipeline_runner_url = Some(format!("{}/{{project}}", runner.uri()));
    let cookie = session_cookie(&config);

    let (status, body) = search(
        config,
        Some(&cookie),
        json!({ "q": "bike", "scope": "Endpoint" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bikes = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "helsinki-bikes")
        .expect("the bikes endpoint");
    assert_eq!(bikes["freshness"]["pipeline"], "hsl-bikes", "{bikes}");
    assert_eq!(bikes["freshness"]["received"], 1200);
    assert_eq!(bikes["freshness"]["errors"], 10);
    assert!(bikes["freshness"]["scrapedAt"]
        .as_str()
        .is_some_and(|s| s.contains('T')));

    let partners = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "helsinki-partners")
        .expect("the partner feed matches 'bike' by its title");
    assert!(
        partners["freshness"].is_null(),
        "nothing feeds it and it is restricted: {partners}"
    );
}

#[tokio::test]
async fn a_space_matched_by_its_own_title_is_open_when_one_of_its_endpoints_is() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let (status, body) = search(
        config,
        Some(&cookie),
        json!({ "q": "context", "scope": "ContextSpace" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(names(&body), vec!["ContextSpace/helsinki"]);
    let space = &body["items"][0];
    assert_eq!(space["access"]["verdict"], "allowed");
    assert!(
        space["access"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("endpoint helsinki-"),
        "{space}"
    );
    assert!(space["matchReason"]
        .as_array()
        .unwrap()
        .contains(&json!("title")));
}

#[test]
fn a_manifest_map_round_trips_through_the_test_helper() {
    // Guards the helper above: a title written into the metadata survives the JSON round trip.
    let env = manifest("Endpoint", "helsinki", "x", Some("Title"), json!({}));
    let meta = serde_json::to_value(&env.metadata).unwrap();
    assert_eq!(meta["title"]["en"], "Title");
    let _: BTreeMap<String, String> = env.metadata.labels.clone();
}
