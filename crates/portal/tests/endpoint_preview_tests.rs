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
    Mock::given(method("POST"))
        .and(path_regex("^/x/views/[a-z0-9-]+/ical/v1/preview$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "read": 3, "valid": 2, "outside": 0, "skipped": {"missing-summary": 1},
            "events": ["BEGIN:VEVENT\r\nSUMMARY:Koncert\r\nEND:VEVENT\r\n"]
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
    /// The admin token file of a converged rig's ViewSync.
    token: Option<std::path::PathBuf>,
}

/// jana reads the space `air`; eva is an organization member who reads nothing of it.
async fn rig(converged: bool) -> Rig {
    rig_with(converged, Config::for_tests()).await
}

/// [`rig`] on a Portal configured as `config` says.
async fn rig_with(converged: bool, config: Config) -> Rig {
    let broker = broker().await;
    let state = AppState::new(config, None);
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
    let mut kept = None;
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
        kept = Some(token);
    }
    Rig {
        state,
        broker,
        token: kept,
    }
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
    // A member of the project's group, whom the gateway's door admits (EP-14).
    let jana = joinedcontext_portal::auth::session::Identity {
        groups: vec!["city".into()],
        ..person("jana")
    };
    let answer = send(&r.state, jana, "POST", &route("air-staff"), Some(draft())).await;
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
            ("x-jc-subject".to_owned(), "sub-jana".to_owned()),
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

/// The MCP door's jc_endpoint_preview reads the page the route reads (T-7921).
#[tokio::test]
async fn the_preview_operation_reads_the_routes_page() {
    use joinedcontext_portal::ops::{self, Caller, Via};
    let r = rig(true).await;
    let op = ops::find("jc_endpoint_preview").expect("registered");
    let mut input = draft();
    input["name"] = json!("air-staff");
    let page = ops::call(op, &Caller::new(member(), Via::Mcp), &r.state, "city", input)
        .await
        .map_err(|err| format!("{err:?}"))
        .expect("the page");
    assert_eq!(
        page,
        json!({"count": 7, "entities": [{"id": "urn:ngsi-ld:Car:1", "type": "Car"}]})
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

/// A Portal whose views have not converged yet answers both previews with the 503 of a view
/// not compiled; one whose broker admin token is gone answers the 503 of a broker that did not
/// answer. Neither reaches the broker.
#[tokio::test]
async fn an_uncompiled_view_or_an_unreachable_broker_is_503_on_both_previews() {
    let mut fresh = rig(false).await;
    fresh.state.views = Some(Arc::new(ViewSync::new(
        fresh.broker.uri(),
        Path::new("/nonexistent/token"),
    )));
    let r = rig(true).await;
    std::fs::remove_file(r.token.as_ref().expect("a converged rig keeps its token"))
        .expect("remove the token");
    for (rig, says) in [(&fresh, "not compiled yet"), (&r, "did not answer")] {
        for (uri, body) in [
            (route("air-staff"), draft()),
            (ical_route("air-staff"), preset(json!({}))),
        ] {
            let answer = send(&rig.state, member(), "POST", &uri, Some(body)).await;
            assert_eq!(
                answer.status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri}: {}",
                answer.text
            );
            assert!(answer.text.contains(says), "{uri} {says}: {}", answer.text);
        }
        assert!(previews(&rig.broker).await.is_empty());
    }
}

/// The preview is a person's editor: jana's bearer token, obtained by a client and verified
/// against the realm, is refused with a 403 that says so, and the broker is never asked.
#[tokio::test]
async fn a_bearer_token_is_refused_the_preview() {
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let r = rig_with(true, config).await;
    let token = common::REALM.person_token_with(
        "portal-api",
        "svc-air",
        "jana",
        &[],
        json!({ "groups": ["/city"] }),
    );
    let response = joinedcontext_portal::server::app(r.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(route("air-staff"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(draft().to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let text = String::from_utf8_lossy(
        &response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes(),
    )
    .into_owned();
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("not a workload token"), "{text}");
    assert!(previews(&r.broker).await.is_empty());
}

/// jana as a member of the project's group, whom the gateway's door admits (EP-14).
fn member() -> joinedcontext_portal::auth::session::Identity {
    joinedcontext_portal::auth::session::Identity {
        groups: vec!["city".into()],
        ..person("jana")
    }
}

/// The broker's answer to the next preview of `kind` (`q` or `x`), over the rig's own 200.
async fn broker_answers(broker: &MockServer, kind: &str, answer: ResponseTemplate) {
    let route = match kind {
        "q" => "^/q/views/[a-z0-9-]+/preview$",
        _ => "^/x/views/[a-z0-9-]+/ical/v1/preview$",
    };
    Mock::given(method("POST"))
        .and(path_regex(route))
        .respond_with(answer)
        .with_priority(1)
        .mount(broker)
        .await;
}

/// EP-85: what the broker refuses reaches the person as the Portal's own status: its 400 with
/// the broker's reason, its 403 as a denial, anything else as a 503 naming the status.
#[tokio::test]
async fn a_draft_preview_carries_the_brokers_refusal() {
    for (answer, want, says) in [
        (
            ResponseTemplate::new(400).set_body_json(json!({"detail": "q: unknown attribute"})),
            StatusCode::BAD_REQUEST,
            "q: unknown attribute",
        ),
        (
            ResponseTemplate::new(403).set_body_json(json!({})),
            StatusCode::FORBIDDEN,
            "no reason given",
        ),
        (
            ResponseTemplate::new(502).set_body_json(json!({})),
            StatusCode::SERVICE_UNAVAILABLE,
            "refused the preview (502)",
        ),
    ] {
        let r = rig(true).await;
        broker_answers(&r.broker, "q", answer).await;
        let answer = send(
            &r.state,
            member(),
            "POST",
            &route("air-staff"),
            Some(draft()),
        )
        .await;
        assert_eq!(answer.status, want, "{}", answer.text);
        assert!(answer.text.contains(says), "{says}: {}", answer.text);
    }
}

/// T-6762: the calendar preview maps the broker's answers the same way, and a 404 means the
/// Endpoint does not serve `ical` yet; a 200 the Portal cannot read is a 503, never a 500.
#[tokio::test]
async fn a_calendar_preview_carries_the_brokers_refusal() {
    for (answer, want, says) in [
        (
            ResponseTemplate::new(400).set_body_json(json!({"title": "bad sort"})),
            StatusCode::BAD_REQUEST,
            "bad sort",
        ),
        (
            ResponseTemplate::new(403).set_body_json(json!({"detail": "not yours"})),
            StatusCode::FORBIDDEN,
            "not yours",
        ),
        (
            ResponseTemplate::new(404).set_body_json(json!({})),
            StatusCode::SERVICE_UNAVAILABLE,
            "does not serve `ical` yet",
        ),
        (
            ResponseTemplate::new(500).set_body_json(json!({})),
            StatusCode::SERVICE_UNAVAILABLE,
            "refused the preview (500)",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"read": "three"})),
            StatusCode::SERVICE_UNAVAILABLE,
            "cannot read",
        ),
        (
            ResponseTemplate::new(200).set_body_string("<html>"),
            StatusCode::SERVICE_UNAVAILABLE,
            "cannot read",
        ),
    ] {
        let r = rig(true).await;
        broker_answers(&r.broker, "x", answer).await;
        let answer = send(
            &r.state,
            member(),
            "POST",
            &ical_route("air-staff"),
            Some(preset(json!({}))),
        )
        .await;
        assert_eq!(answer.status, want, "{}", answer.text);
        assert!(answer.text.contains(says), "{says}: {}", answer.text);
    }
}

/// A calendar preview asks for 1 to 50 events; past either end is a 422 naming `limit`, and
/// without a broker address the preview is a 503 that says so.
#[tokio::test]
async fn a_calendar_preview_limit_is_1_to_50_and_needs_a_broker() {
    let r = rig(true).await;
    for limit in [0, 51] {
        let mut body = preset(json!({}));
        body["limit"] = json!(limit);
        let answer = send(
            &r.state,
            member(),
            "POST",
            &ical_route("air-staff"),
            Some(body),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{limit}: {}",
            answer.text
        );
        assert!(answer.text.contains("1 to 50"), "{}", answer.text);
    }
    let bare = rig(false).await;
    let answer = send(
        &bare.state,
        member(),
        "POST",
        &ical_route("air-staff"),
        Some(preset(json!({}))),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    assert!(answer.text.contains("no broker address"), "{}", answer.text);
}

fn ical_route(name: &str) -> String {
    format!("/api/v1/projects/city/endpoints/{name}/ical-preview")
}

fn preset(extra: Value) -> Value {
    let mut output = json!({"name": "kultura", "query": "q=eventCategory==%22koncert%22",
        "sources": [{"type": "Event", "preset": "sdm-event"}]});
    if let (Some(o), Some(extra)) = (output.as_object_mut(), extra.as_object()) {
        o.extend(extra.clone());
    }
    json!({ "output": output })
}

/// T-6762 (API/02 §4a): the form previews one calendar preset through the saved view as the
/// person; the broker gets the preset's `query` as the `filter` it stands for, with its sort,
/// limit and zone, and the answer is the broker's counts and first entries.
#[tokio::test]
async fn a_member_previews_a_calendar_preset_through_the_saved_view() {
    let r = rig(true).await;
    let kept = json!({"sort": "-start", "limit": 5, "timezone": "Europe/Prague"});
    // a member of the project's group, whom the gateway's door admits (EP-14)
    let jana = joinedcontext_portal::auth::session::Identity {
        groups: vec!["city".into()],
        ..person("jana")
    };
    let answer = send(
        &r.state,
        jana,
        "POST",
        &ical_route("air-staff"),
        Some(preset(kept)),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    assert_eq!(
        (body["read"].clone(), body["valid"].clone()),
        (json!(3), json!(2))
    );
    assert_eq!(body["skipped"], json!({"missing-summary": 1}));
    let asked = previews(&r.broker).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let (headers, _, sent) = &asked[0];
    assert!(
        headers.contains(&("x-jc-subject".to_owned(), "sub-jana".to_owned())),
        "{headers:?}"
    );
    assert_eq!(
        sent["output"]["filter"],
        json!({"q": "eventCategory==\"koncert\""})
    );
    assert!(sent["output"].get("query").is_none(), "{sent}");
    assert_eq!(
        (
            sent["output"]["sort"].clone(),
            sent["output"]["limit"].clone(),
            sent["output"]["timezone"].clone()
        ),
        (json!("-start"), json!(5), json!("Europe/Prague"))
    );
}

/// Each preset field is checked again on the server: a refusal is 422 naming the member, and
/// the broker is never asked; somebody the space does not admit gets the 404 of no Endpoint.
#[tokio::test]
async fn a_calendar_preset_preview_rechecks_each_field() {
    let r = rig(true).await;
    for (extra, member) in [
        (json!({"sort": "title"}), "`sort: title`"),
        (json!({"limit": 0}), "`limit`"),
        (json!({"limit": 10001}), "`limit`"),
        (json!({"timezone": "Mars/Base; rm"}), "`timezone"),
        (json!({"query": "limit=5"}), "`query` names `limit`"),
    ] {
        let answer = send(
            &r.state,
            person("jana"),
            "POST",
            &ical_route("air-staff"),
            Some(preset(extra)),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            answer.text
        );
        assert!(answer.text.contains(member), "{member}: {}", answer.text);
    }
    let answer = send(
        &r.state,
        person("jana"),
        "POST",
        &ical_route("air-staff"),
        Some(json!({"output": preset(json!({}))["output"], "limit": 51})),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    let refused = send(
        &r.state,
        person("eva"),
        "POST",
        &ical_route("air-staff"),
        Some(preset(json!({}))),
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    assert!(previews(&r.broker).await.is_empty());
}
