//! T-0453: the drift surface (CC-21, CC-38, UI-25, UI-26; API/01 §20).
//!
//! Configuration cannot drift — every component reads it from the repository (CC-72, and
//! T-0421 settled it) — so what the routes serve is one thing: the seed entities a space
//! declares, against what it holds. The reconciler scans on its tick and these read what it
//! found, which is why a page renders without waiting on a broker.

mod common;
use common::csrf_echo::EchoCsrf;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::reconciler::drift::{Difference, Drifted, Found, Kind, Store};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tower::ServiceExt;

const PROJECT: &str = "helsinki";
const SPACE: &str = "helsinki";
const ENTITY: &str = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-1";

fn cookies(config: &Config, groups: Vec<String>) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@hel.fi".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups,
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        stepped_up_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!(
        "{CSRF_COOKIE}={}",
        joinedcontext_portal::auth::csrf::Token::bound(&config.cookie_key, &session).as_str()
    ));
    parts.join("; ")
}

/// What one scan found: one entity the space answers differently, one it does not hold.
fn scanned() -> Arc<Store> {
    let store = Arc::new(Store::default());
    let mut found = BTreeMap::new();
    found.insert(
        PROJECT.to_owned(),
        Found {
            observed_at: "2026-09-18T09:12:03Z".parse().expect("an instant"),
            entities: vec![
                Drifted {
                    space: SPACE.to_owned(),
                    id: ENTITY.to_owned(),
                    drift: Kind::Modified,
                    diff: vec![Difference {
                        path: "airQualityIndex.value".to_owned(),
                        declared: json!(42),
                        live: Some(json!(7)),
                    }],
                    resolutions: vec!["revert", "adopt"],
                    source: "projects/helsinki/spaces/helsinki/entities/seed/stations.json"
                        .to_owned(),
                    declared: json!({ "id": ENTITY, "type": "AirQualityObserved" }),
                },
                Drifted {
                    space: SPACE.to_owned(),
                    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2".to_owned(),
                    drift: Kind::Missing,
                    diff: Vec::new(),
                    resolutions: vec!["revert"],
                    source: "projects/helsinki/spaces/helsinki/entities/seed/stations.json"
                        .to_owned(),
                    declared: json!({ "id": "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2" }),
                },
            ],
        },
    );
    store.replace_all(found);
    store
}

async fn call(
    method: &str,
    uri: &str,
    store: Option<Arc<Store>>,
    groups: Vec<String>,
) -> (StatusCode, Value) {
    let config = Config::for_tests();
    let cookie = cookies(&config, groups);
    let mut state = AppState::new(config, None);
    if let Some(store) = store {
        state.drift = store;
    }
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .echo_csrf()
        .body(Body::empty())
        .expect("request");
    let response = server::app(state).oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, value)
}

fn steward() -> Vec<String> {
    vec!["portal-approver".to_owned()]
}

#[tokio::test]
async fn the_list_is_what_the_last_scan_found_and_says_when_it_ran() {
    let (status, body) = call(
        "GET",
        &format!("/api/v1/projects/{PROJECT}/drift"),
        Some(scanned()),
        steward(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["metadata"]["observedAt"],
        json!("2026-09-18T09:12:03+00:00")
    );
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{body}");

    let modified = &items[0];
    assert_eq!(modified["drift"], json!("MODIFIED"));
    assert_eq!(modified["diff"][0]["path"], json!("airQualityIndex.value"));
    assert_eq!(modified["diff"][0]["declared"], json!(42));
    assert_eq!(modified["diff"][0]["live"], json!(7));
    // Exactly the two resolutions UI-26 puts on the screen, and only the one that is possible
    // where the entity is not there to adopt.
    assert_eq!(modified["resolutions"], json!(["revert", "adopt"]));
    assert_eq!(items[1]["drift"], json!("MISSING"));
    assert_eq!(items[1]["resolutions"], json!(["revert"]));
    assert!(items[1]["diff"].as_array().expect("diff").is_empty());

    // The whole declared entity is the revert's business and nobody else's: the page reads the
    // file it names, and an entity on the wire would be the file twice.
    assert!(modified.get("declared").is_none(), "{modified}");
    assert!(
        modified["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("projects/")),
        "{modified}"
    );
}

#[tokio::test]
async fn a_project_nobody_has_scanned_says_so_rather_than_answering_clean() {
    let (status, body) = call(
        "GET",
        &format!("/api/v1/projects/{PROJECT}/drift"),
        None,
        steward(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"], json!([]));
    // "Nothing drifted" and "nothing was read" are not the same answer, and the page must be
    // able to tell them apart.
    assert!(
        body["metadata"].get("observedAt").is_none(),
        "an unscanned project claimed an observation: {body}"
    );
}

#[tokio::test]
async fn a_resolution_the_scan_did_not_report_is_not_a_write_nobody_asked_for() {
    for verb in ["revert", "adopt"] {
        let (status, body) = call(
            "POST",
            &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/urn:ngsi-ld:Nothing:hel.fi:helsinki:x/{verb}"),
            Some(scanned()),
            steward(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{verb}: {body}");
    }
}

#[tokio::test]
async fn an_entity_the_space_does_not_hold_cannot_be_adopted() {
    let missing = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2";
    let (status, body) = call(
        "POST",
        &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{missing}/adopt"),
        Some(scanned()),
        steward(),
    )
    .await;

    // Adopting "it is gone" means proposing an empty file, which is a deletion and goes through
    // the explicit-deletion path rather than through a button (CC-19, UI-26).
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("explicit deletion"), "{body}");
}

#[tokio::test]
async fn a_reader_reads_and_resolves_nothing() {
    // No binding at all: the project is not theirs to see, and the route says "not found"
    // rather than "forbidden", which would say which projects exist (R20).
    let (status, _) = call(
        "GET",
        &format!("/api/v1/projects/{PROJECT}/drift"),
        Some(scanned()),
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(
        "POST",
        &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{ENTITY}/revert"),
        Some(scanned()),
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_resolution_without_a_space_surface_refuses_instead_of_writing_nowhere() {
    let (status, body) = call(
        "POST",
        &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{ENTITY}/revert"),
        Some(scanned()),
        steward(),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
}

/// A person granted `verbs` on Entity in the project, or only in `space` when one is named.
fn granted(
    state: &AppState,
    verbs: Value,
    space: Option<&str>,
) -> joinedcontext_portal::auth::session::Identity {
    // A binding scoped to a space names it; the space's project is the one holding it.
    let scope = match space {
        Some(space) => {
            state
                .mirror
                .upsert(common::envelope("ContextSpace", space, PROJECT, json!({})));
            json!({ "contextSpace": space })
        }
        None => json!({ "project": PROJECT }),
    };
    state.mirror.upsert(common::envelope(
        "Role",
        "resolver",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Entity", "ContextSpace"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "resolvers",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "resolver@hel.fi" }], "role": "resolver", "scope": scope }),
    ));
    common::person("resolver")
}

/// T-7657: a reader bound to one space sees that space's drift and no other, with the instant
/// of the scan still there.
#[tokio::test]
async fn a_reader_of_one_space_sees_only_that_spaces_drift() {
    let mut state = AppState::new(Config::for_tests(), None);
    state.drift = scanned();
    let uri = format!("/api/v1/projects/{PROJECT}/drift");
    let own = granted(&state, json!(["read"]), Some(SPACE));
    let answer = common::send(&state, own, "GET", &uri, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("a list");
    assert_eq!(list["items"].as_array().map(Vec::len), Some(2), "{list}");

    let elsewhere = granted(&state, json!(["read"]), Some("vantaa-air"));
    let answer = common::send(&state, elsewhere, "GET", &uri, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("a list");
    assert_eq!(list["items"], json!([]), "{list}");
    assert_eq!(list["metadata"]["observedAt"], "2026-09-18T09:12:03+00:00");
}

/// T-7657: revert upserts what the file declares into the space with the Portal's own token and
/// answers 204; a space that refuses the write is 503 and says which space.
#[tokio::test]
async fn revert_writes_the_declared_entity_back_and_a_refusing_space_is_503() {
    use joinedcontext_portal::auth::client_auth::ClientAuth;
    use joinedcontext_portal::reconciler::drift::Watch;
    use wiremock::matchers::{bearer_token, body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let space = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/city/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&space)
        .await;
    let mut state = AppState::new(Config::for_tests(), None);
    state.drift = scanned();
    state.drift_watch = Some(Arc::new(Watch::new(
        space.uri(),
        &format!("{}/realms/city", space.uri()),
        "portal".to_owned(),
        ClientAuth::Secret("s".into()),
    )));
    let who = granted(&state, json!(["read", "propose"]), None);
    let uri = format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{ENTITY}/revert");

    let refusal = Mock::given(method("POST"))
        .and(path(format!(
            "/cs/{SPACE}/ngsi-ld/v1/entityOperations/upsert"
        )))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount_as_scoped(&space)
        .await;
    let answer = common::send(&state, who.clone(), "POST", &uri, None).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("helsinki refused the write"),
        "{}",
        answer.text
    );
    drop(refusal);

    Mock::given(method("POST"))
        .and(path(format!(
            "/cs/{SPACE}/ngsi-ld/v1/entityOperations/upsert"
        )))
        .and(bearer_token("t"))
        .and(body_string_contains(ENTITY))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&space)
        .await;
    let answer = common::send(&state, who, "POST", &uri, None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    space.verify().await;
}

/// One entity the space answers differently, declared in `source`.
fn drifted_in(source: &str) -> Arc<Store> {
    let store = Arc::new(Store::default());
    let mut found = BTreeMap::new();
    found.insert(
        PROJECT.to_owned(),
        Found {
            observed_at: "2026-09-18T09:12:03Z".parse().expect("an instant"),
            entities: vec![Drifted {
                space: SPACE.to_owned(),
                id: ENTITY.to_owned(),
                drift: Kind::Modified,
                diff: Vec::new(),
                resolutions: vec!["revert", "adopt"],
                source: source.to_owned(),
                declared: json!({ "id": ENTITY, "type": "AirQualityObserved" }),
            }],
        },
    );
    store.replace_all(found);
    store
}

/// An adoption by a person granted propose on Entity, with their own token, against `gateway`
/// (which serves the live entity) and, when `forge` is set, the repository on that mock.
async fn adopt(
    gateway: &wiremock::MockServer,
    forge: bool,
    store: Arc<Store>,
) -> (StatusCode, String) {
    let gateway_url = gateway.uri();
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_PORTAL_GATEWAY_URL" => Some(gateway_url.as_str()),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let mut state = AppState::new(config, None);
    if forge {
        let client = joinedcontext_portal::forge::ShakoClient::new(
            gateway_url.parse().expect("mock url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("client");
        state = state.with_shako(Arc::new(client));
    }
    state.drift = store;
    state.mirror.upsert(common::envelope(
        "Role",
        "adopter",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Entity", "ContextSpace"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "adopters",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "adopter@hel.fi" }], "role": "adopter",
                "scope": { "project": PROJECT } }),
    ));
    let token = common::REALM.person_token_of("portal-api", "portal-api", "adopter", &[]);
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/drift/{SPACE}/{ENTITY}/adopt"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// T-7657: adopting needs a seed file of the entity's own space, a forge, the file still in the
/// repository and the entity still in the file; each missing piece is refused with what to do,
/// and nothing is written.
#[tokio::test]
async fn adopting_refuses_a_file_that_is_no_seed_gone_or_without_the_entity() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let seed = "projects/helsinki/spaces/helsinki/entities/seed/stations.json";
    let live = || {
        Mock::given(method("GET"))
            .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/entities/{ENTITY}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": ENTITY, "type": "AirQualityObserved",
                "airQualityIndex": { "type": "Property", "value": 7 }
            })))
    };

    let gateway = MockServer::start().await;
    live().mount(&gateway).await;
    for source in [
        "projects/helsinki/spaces/helsinki/entities/stations.yaml",
        "projects/helsinki/spaces/vantaa/entities/seed/stations.json",
    ] {
        let (status, text) = adopt(&gateway, false, drifted_in(source)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{source}: {text}");
        assert!(text.contains("is no seed file of space"), "{text}");
    }
    let (status, text) = adopt(&gateway, false, drifted_in(seed)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("forge is not configured"), "{text}");

    let gone = common::forge().await;
    live().mount(&gone).await;
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .and(query_param("@p", format!("'{seed}'")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&gone)
        .await;
    let (status, text) = adopt(&gone, true, drifted_in(seed)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("is no longer in the repository"), "{text}");

    let other = common::forge().await;
    live().mount(&other).await;
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "blob-1",
            "Content": common::encode(r#"{ "id": "urn:ngsi-ld:AirQualityObserved:other", "type": "AirQualityObserved" }"#)
        })))
        .mount(&other)
        .await;
    let (status, text) = adopt(&other, true, drifted_in(seed)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("no longer declares"), "{text}");
    let sent = other.received_requests().await.unwrap_or_default();
    assert!(
        !sent.iter().any(|r| r.method.as_str() == "POST"),
        "nothing written: {:?}",
        sent.iter()
            .map(|r| r.url.path().to_owned())
            .collect::<Vec<_>>()
    );
}

/// Adopting writes the live values of the attributes the seed file declares into that file, on a
/// branch of its own, and opens the merge request an approver decides (F-POR4-004): what the
/// space holds beside them and what only the broker keeps stay out, and the other entity of the
/// file is untouched.
#[tokio::test]
async fn adopting_proposes_the_seed_file_with_the_live_values_as_a_change() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, ResponseTemplate};

    let forge = common::forge().await;
    let source = "projects/helsinki/spaces/helsinki/entities/seed/stations.json";
    let other = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2";
    let file = json!([
        { "id": ENTITY, "type": "AirQualityObserved",
          "airQualityIndex": { "type": "Property", "value": 42 },
          "retired": { "type": "Property", "value": false } },
        { "id": other, "type": "AirQualityObserved" }
    ]);
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .and(query_param("@p", format!("'{source}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "blob-1",
            "Content": common::encode(&file.to_string())
        })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/entities/{ENTITY}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": ENTITY, "type": "AirQualityObserved",
            "airQualityIndex": { "type": "Property", "value": 7,
                                 "modifiedAt": "2026-10-26T08:00:00Z" },
            "batteryLevel": { "type": "Property", "value": 0.4 }
        })))
        .mount(&forge)
        .await;

    // The live entity is read through the gateway (here the forge mock) with the person's own
    // token, never with the Portal's account (T-5698).
    let forge_url = forge.uri();
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_PORTAL_GATEWAY_URL" => Some(forge_url.as_str()),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let client = joinedcontext_portal::forge::ShakoClient::new(
        forge_url.parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let mut state = AppState::new(config, None).with_shako(Arc::new(client));
    state.drift = scanned();
    let answer = common::send(
        &state,
        common::person("adopter"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{ENTITY}/adopt"),
        None,
    )
    .await;
    // Nobody grants this person anything in the project yet: it is not there for them.
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);

    state.mirror.upsert(common::envelope(
        "Role",
        "adopter",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Entity", "ContextSpace"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "adopters",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "adopter@hel.fi" }], "role": "adopter",
                "scope": { "project": PROJECT } }),
    ));
    let token = common::REALM.person_token_of("portal-api", "portal-api", "adopter", &[]);
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/drift/{SPACE}/{ENTITY}/adopt"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    let change: Value = serde_json::from_str(&text).expect("a Change");
    assert_eq!(change["metadata"]["name"], "chg-00000009", "{change}");
    assert_eq!(change["status"]["phase"], "PendingApproval");

    let sent = forge.received_requests().await.unwrap_or_default();
    let read = sent
        .iter()
        .find(|r| r.url.path().starts_with(&format!("/cs/{SPACE}/")))
        .expect("the live entity read");
    assert_eq!(
        read.headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some(format!("Bearer {token}").as_str()),
        "read with the person's own token"
    );
    assert!(
        !sent
            .iter()
            .any(|r| r.url.path().ends_with("/openid-connect/token")),
        "the Portal fetched no token of its own"
    );
    let branch = sent
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.CreateBranch"))
        .expect("a branch");
    assert!(
        String::from_utf8_lossy(&branch.body).contains("portal/adopt-"),
        "{}",
        String::from_utf8_lossy(&branch.body)
    );
    let commit = sent
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.CommitFiles"))
        .expect("one commit");
    let commit: Value = serde_json::from_slice(&commit.body).expect("a json commit");
    let written = &commit["Files"][0];
    assert_eq!(written["path"], source);
    assert_eq!(written["operation"], "update");
    assert_eq!(written["sha"], "blob-1");
    use base64::Engine;
    let text = base64::engine::general_purpose::STANDARD
        .decode(written["content"].as_str().expect("content"))
        .expect("base64");
    let adopted: Value = serde_json::from_slice(&text).expect("json");
    assert_eq!(
        adopted,
        json!([
            { "id": ENTITY, "type": "AirQualityObserved",
              "airQualityIndex": { "type": "Property", "value": 7 } },
            { "id": other, "type": "AirQualityObserved" }
        ])
    );
    assert!(sent
        .iter()
        .any(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.OpenPullRequest")));
}
