//! Previewing an Endpoint's filter (API/01 §40, EP-85, T-3802): the Portal compiles the draft
//! into the Endpoint's candidate view and the broker's view preview reads the page through it as
//! the person, or as the anonymous caller on a public Endpoint; only a member of the space asks.

mod common;

use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::reconciler::views::ViewSync;
use joinedcontext_portal::state::AppState;

const STAFF: &str = "w3nq7pzk2mtv6hx5brj4gcdsfa";
const PUBLIC: &str = "k4y7pq2mzt6vhx3nbwrs5cjdaf";
const TOKEN: &str = "an-admin-token-of-at-least-32-bytes!";

/// The repository: the space `air` of `city`, Cars readable with `speed` and `owner`, and two
/// Endpoints on it, one for the organization and one public.
fn repository(dir: &Path) -> jcctl::loader::Repository {
    let write = |rel: &str, body: String| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    };
    let head = |kind: &str, name: &str| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: city\nspec:\n")
    };
    write(
        "projects/city/spaces/air/space.yaml",
        format!("{}  isSandbox: false\n", head("ContextSpace", "air")),
    );
    for (name, slug, audience) in [
        ("air-staff", STAFF, "organization"),
        ("air-public", PUBLIC, "public"),
    ] {
        write(
            &format!("projects/city/spaces/air/endpoints/{name}.yaml"),
            format!("{}  contextSpaceRef: air\n  slug: {slug}\n  audience: {audience}\n  enabledRepresentations: [ngsi-ld]\n", head("Endpoint", name)),
        );
    }
    write(
        "projects/city/spaces/air/policies/cars.yaml",
        format!("{}  contextSpaceRef: air\n  assigner: did:web:city.example\n  assignee: {{ kind: role, id: public }}\n  operations: [retrieveOps]\n  information:\n    - entities: [{{ type: Car }}]\n      propertyNames: [speed, owner]\n", head("Policy", "cars")),
    );
    jcctl::loader::Repository::load(dir).expect("the repository loads")
}

/// A broker that takes every view and answers every preview with one Car.
async fn broker() -> MockServer {
    let broker = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/q/views"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&broker)
        .await;
    Mock::given(method("POST"))
        .and(path_regex("^/q/views/[a-z0-9-]+/preview$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 7, "entities": [{"id": "urn:ngsi-ld:Car:1", "type": "Car"}]
        })))
        .mount(&broker)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&broker)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&broker)
        .await;
    broker
}

struct Rig {
    state: AppState,
    broker: MockServer,
}

/// jana reads the space `air`; eva is an organization member who reads nothing of it.
async fn rig(converged: bool) -> Rig {
    let broker = broker().await;
    let state = AppState::new(Config::for_tests(), None);
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", "city", json!({})));
    for (name, slug, audience) in [
        ("air-staff", STAFF, "organization"),
        ("air-public", PUBLIC, "public"),
    ] {
        state.mirror.upsert(envelope(
            "Endpoint",
            name,
            "city",
            json!({ "contextSpaceRef": "air", "slug": slug, "audience": audience }),
        ));
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
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "space-reader",
                "scope": { "contextSpace": "air" } }),
    ));
    let mut state = state;
    if converged {
        let dir = std::env::temp_dir().join(format!(
            "portal-preview-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let token = dir.join("token");
        let repo = repository(&dir);
        std::fs::write(&token, format!("{TOKEN}\n")).expect("write the token");
        let views = ViewSync::new(broker.uri(), &token);
        views.converge(&repo).await;
        state.views = Some(Arc::new(views));
    }
    Rig { state, broker }
}

/// The preview requests the broker received: the headers named, the query and the view.
async fn previews(broker: &MockServer) -> Vec<(Vec<(String, String)>, String, Value)> {
    broker
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/preview"))
        .map(|request| {
            let headers = request
                .headers
                .iter()
                .filter(|(name, _)| name.as_str().starts_with("x-jc-"))
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        value.to_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect();
            let query = request.url.query().unwrap_or_default().to_owned();
            let body = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            (headers, query, body)
        })
        .collect()
}

fn route(name: &str) -> String {
    format!("/api/v1/projects/city/endpoints/{name}/preview")
}

fn draft() -> Value {
    json!({
        "projection": { "classes": [{ "name": "Car", "slots": ["speed", "owner"] }],
                        "filter": { "q": "speed>0" } },
        "hiddenAttributes": ["owner"],
        "type": "Car", "id": ["urn:ngsi-ld:Car:1"], "limit": 20, "offset": 40
    })
}

/// EP-85: the person's own reading through the draft's view: the projection's row filter, the
/// hidden attribute out of its rules, the page as asked; the answer is the broker's count and page.
#[tokio::test]
async fn a_member_previews_the_draft_as_themselves() {
    let r = rig(true).await;
    let answer = send(
        &r.state,
        person("jana"),
        "POST",
        &route("air-staff"),
        Some(draft()),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    assert_eq!(
        body,
        json!({"count": 7, "entities": [{"id": "urn:ngsi-ld:Car:1", "type": "Car"}]})
    );
    let asked = previews(&r.broker).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let (headers, query, view) = &asked[0];
    assert_eq!(
        headers,
        &vec![
            ("x-jc-subject".to_owned(), "jana".to_owned()),
            ("x-jc-roles".to_owned(), String::new()),
        ]
    );
    assert_eq!(
        query,
        "type=Car&id=urn%3Angsi-ld%3ACar%3A1&limit=20&offset=40"
    );
    assert_eq!(view["id"], STAFF);
    assert_eq!(view["q"], "speed>0");
    assert_eq!(
        view["rules"],
        json!([{"types": ["Car"], "attributes": ["speed"]}])
    );
}

/// EP-16: a public Endpoint's draft is read as the anonymous caller, never as the person.
#[tokio::test]
async fn a_public_endpoints_draft_is_read_as_the_public() {
    let r = rig(true).await;
    let answer = send(
        &r.state,
        person("jana"),
        "POST",
        &route("air-public"),
        Some(draft()),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let asked = previews(&r.broker).await;
    assert_eq!(
        asked.first().map(|(headers, _, _)| headers.clone()),
        Some(vec![("x-jc-roles".to_owned(), "public".to_owned())])
    );
}

/// SP-06, SP-11: somebody the space does not admit learns nothing, not even that the Endpoint
/// is there, and the broker is never asked.
#[tokio::test]
async fn a_non_member_gets_the_404_of_an_unknown_endpoint() {
    let r = rig(true).await;
    let refused = send(
        &r.state,
        person("eva"),
        "POST",
        &route("air-staff"),
        Some(draft()),
    )
    .await;
    let unknown = send(
        &r.state,
        person("jana"),
        "POST",
        &route("nothing"),
        Some(draft()),
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    assert!(previews(&r.broker).await.is_empty());
}

/// A refused draft names the member at fault and asks the broker nothing; a Portal that has
/// compiled no view answers 503.
#[tokio::test]
async fn a_bad_draft_is_400_and_no_view_is_503() {
    let r = rig(true).await;
    for (body, member) in [
        (json!({"type": "Car", "colour": "red"}), "colour"),
        (json!({"type": "Car", "limit": 101}), "limit"),
        (
            json!({"type": "Car", "hiddenAttributes": ["id"]}),
            "hiddenAttributes",
        ),
        (
            json!({"type": "Car", "projection": {"classes": []}}),
            "projection",
        ),
    ] {
        let answer = send(
            &r.state,
            person("jana"),
            "POST",
            &route("air-staff"),
            Some(body),
        )
        .await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
        assert!(answer.text.contains(member), "{member}: {}", answer.text);
    }
    assert!(previews(&r.broker).await.is_empty());

    let r = rig(false).await;
    let answer = send(
        &r.state,
        person("jana"),
        "POST",
        &route("air-staff"),
        Some(draft()),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
}
