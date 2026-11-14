//! The sources routes of a `kind: Portal` (ODP-15, T-6883): test a source, run it now, list its
//! runs and the portal's datasets. Another Organization's portal is the same 404 as an unknown
//! one, a member without the right is 403, and every call reaches only the portal it names.

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use jc_core::kinds::PortalSpec;
use serde_json::{json, Value};
use tower::ServiceExt;

use common::doors::http;
use common::envelope;
use joinedcontext_portal::api::portal_sources::{
    hand_desk_to, DeskDataset, DeskError, DeskFuture, SourceDesk,
};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

/// A desk whose catalogue of each portal holds one dataset named after the portal, and whose
/// runs stay active until the test ends.
#[derive(Default)]
struct Fake {
    active: Mutex<BTreeSet<(String, String, String)>>,
}

fn key(portal: &PortalSpec, source: &str) -> (String, String, String) {
    (
        portal.organization_ref.clone(),
        portal.path.clone(),
        source.to_owned(),
    )
}

impl SourceDesk for Fake {
    fn test<'a>(&'a self, _portal: &'a PortalSpec, _source: &'a str) -> DeskFuture<'a, String> {
        Box::pin(async { Ok("ok".to_owned()) })
    }
    fn start<'a>(&'a self, portal: &'a PortalSpec, source: &'a str) -> DeskFuture<'a, ()> {
        let fresh = self.active.lock().unwrap().insert(key(portal, source));
        Box::pin(async move { fresh.then_some(()).ok_or(DeskError::Busy) })
    }
    fn runs<'a>(&'a self, portal: &'a PortalSpec, source: &'a str) -> DeskFuture<'a, Vec<Value>> {
        let state = if self.active.lock().unwrap().contains(&key(portal, source)) {
            "running"
        } else {
            "succeeded"
        };
        Box::pin(async move { Ok(vec![json!({ "id": "run-1", "state": state })]) })
    }
    fn datasets<'a>(&'a self, portal: &'a PortalSpec) -> DeskFuture<'a, Vec<DeskDataset>> {
        let own = format!("{}-{}", portal.organization_ref, portal.path);
        Box::pin(async move {
            Ok(vec![
                DeskDataset {
                    name: format!("{own}-air"),
                    title: "Air".into(),
                    source: Some("kosice".into()),
                },
                DeskDataset {
                    name: format!("{own}-hidden"),
                    title: "Hidden".into(),
                    source: None,
                },
            ])
        })
    }
}

fn fake() -> &'static Arc<Fake> {
    static FAKE: OnceLock<Arc<Fake>> = OnceLock::new();
    FAKE.get_or_init(|| {
        let fake = Arc::new(Fake::default());
        hand_desk_to(fake.clone());
        fake
    })
}

fn person(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn bind(m: &Mirror, name: &str, who: &str, role: &str, org: &str) {
    m.upsert(envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@example.org") }], "role": role,
            "scope": { "organization": org } }),
    ));
}

/// The installation and two Organizations, `bbsk` and `praha`, each with an administrator and a
/// portal `skoly` holding a dcat-ap source `kosice` and an `endpoints` source `own`; `bbsk` has a
/// reader of its portals (`bbreader`) and a member who reads no portal (`bbother`).
fn world() -> AppState {
    fake();
    let mut config = Config::for_tests();
    config.portal_settings =
        joinedcontext_portal::portals::parse("", Some("6"), "data.kosice.sk").expect("settings");
    let state = AppState::new(config, None).with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    for (name, spec) in [
        ("ops", json!({ "domain": "ops.example", "installation": true })),
        ("bbsk", json!({ "domain": "bbsk.sk" })),
        ("praha", json!({ "domain": "praha.eu" })),
    ] {
        m.upsert(envelope("Organization", name, ORG_NAMESPACE, spec));
    }
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    for (role, rule) in [
        ("org-admin", json!({ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] })),
        ("portal-reader", json!({ "kinds": ["Portal"], "verbs": ["read"] })),
        ("group-reader", json!({ "kinds": ["Group"], "verbs": ["read"] })),
    ] {
        m.upsert(envelope("Role", role, ORG_NAMESPACE, json!({ "rules": [rule] })));
    }
    for (who, org) in [("bbadmin", "bbsk"), ("prahaadmin", "praha")] {
        bind(m, &format!("{org}-admins"), who, "org-admin", org);
        m.upsert(envelope(
            "Portal",
            &format!("{org}-skoly"),
            ORG_NAMESPACE,
            json!({ "organizationRef": org, "path": "skoly", "hidden": [format!("{org}-skoly-hidden")],
                "sources": [
                    { "name": "kosice", "type": "dcat-ap", "url": "https://data.kosice.sk/catalog.ttl",
                      "allowedHosts": ["data.kosice.sk"], "schedule": "23 3 * * *",
                      "budget": { "records": 100, "bytes": 1000000 } },
                    { "name": "own", "type": "endpoints" }
                ] }),
        ));
    }
    bind(m, "bbsk-readers", "bbreader", "portal-reader", "bbsk");
    bind(m, "bbsk-others", "bbother", "group-reader", "bbsk");
    state
}

fn source_route(portal: &str, source: &str, end: &str) -> String {
    format!("/api/v1/projects/{ORG_NAMESPACE}/portals/{portal}/sources/{source}/{end}")
}

fn datasets_route(portal: &str) -> String {
    format!("/api/v1/projects/{ORG_NAMESPACE}/portals/{portal}/datasets")
}

/// Every route, as (method, uri) of portal `portal`.
fn routes(portal: &str) -> [(&'static str, String); 4] {
    [
        ("POST", source_route(portal, "kosice", "test")),
        ("POST", source_route(portal, "kosice", "runs")),
        ("GET", source_route(portal, "kosice", "runs")),
        ("GET", datasets_route(portal)),
    ]
}

#[tokio::test]
async fn nobody_signed_in_reaches_a_source_route() {
    let state = world();
    for (method, uri) in routes("bbsk-skoly") {
        let response = joinedcontext_portal::server::app(state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        // A POST without a session meets the CSRF door first: 403, never a call.
        let refused = match method {
            "GET" => response.status() == StatusCode::UNAUTHORIZED,
            _ => matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
        };
        assert!(refused, "{method} {uri}: {}", response.status());
    }
}

#[tokio::test]
async fn another_organizations_portal_is_the_same_404_as_none() {
    let state = world();
    for ((method, theirs), (_, unknown)) in routes("praha-skoly").into_iter().zip(routes("bbsk-nic")) {
        let a = http(&state, person("bbadmin"), method, &theirs, None).await;
        let b = http(&state, person("bbadmin"), method, &unknown, None).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{method} {theirs}: {}", a.body);
        assert_eq!(b.status, StatusCode::NOT_FOUND, "{method} {unknown}: {}", b.body);
        let detail = |body: &Value| {
            body["detail"]
                .as_str()
                .unwrap_or_default()
                .replace("praha-skoly", "X")
                .replace("bbsk-nic", "X")
        };
        assert_eq!(detail(&a.body), detail(&b.body), "{method}: same body");
    }
    let unknown_source = http(
        &state,
        person("bbadmin"),
        "GET",
        &source_route("bbsk-skoly", "nic", "runs"),
        None,
    )
    .await;
    assert_eq!(unknown_source.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_member_without_the_right_is_403() {
    let state = world();
    // A reader of the portal may list, never test or run.
    for (method, uri) in routes("bbsk-skoly") {
        let reader = http(&state, person("bbreader"), method, &uri, None).await;
        let expected = if method == "POST" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        };
        assert_eq!(reader.status, expected, "{method} {uri}: {}", reader.body);
        let other = http(&state, person("bbother"), method, &uri, None).await;
        assert_eq!(other.status, StatusCode::FORBIDDEN, "{method} {uri}: {}", other.body);
    }
}

#[tokio::test]
async fn an_endpoints_source_has_nothing_to_test_or_run() {
    let state = world();
    for end in ["test", "runs"] {
        let r = http(
            &state,
            person("prahaadmin"),
            "POST",
            &source_route("praha-skoly", "own", end),
            None,
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{end}: {}", r.body);
    }
}

#[tokio::test]
async fn test_connection_is_rate_limited_per_person() {
    let state = world();
    let uri = source_route("bbsk-skoly", "kosice", "test");
    let mut admin = person("bbadmin");
    admin.subject = "sub-rate-limited-admin".into();
    for _ in 0..3 {
        let ok = http(&state, admin.clone(), "POST", &uri, None).await;
        assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
        assert_eq!(ok.body, json!({ "outcome": "ok" }));
    }
    let config = state.config.clone();
    let limited = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(header::COOKIE, common::cookie(&config, admin))
                .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().get(header::RETRY_AFTER).is_some());
    // Another person keeps their own tests.
    let other = http(&state, person("prahaadmin"), "POST", &source_route("praha-skoly", "kosice", "test"), None).await;
    assert_eq!(other.status, StatusCode::OK, "{}", other.body);
}

#[tokio::test]
async fn run_now_twice_is_409() {
    let state = world();
    let uri = source_route("praha-skoly", "kosice", "runs");
    let first = http(&state, person("prahaadmin"), "POST", &uri, None).await;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.body);
    let second = http(&state, person("prahaadmin"), "POST", &uri, None).await;
    assert_eq!(second.status, StatusCode::CONFLICT, "{}", second.body);
    let runs = http(&state, person("prahaadmin"), "GET", &uri, None).await;
    assert_eq!(runs.body["items"][0]["state"], "running", "{}", runs.body);
}

#[tokio::test]
async fn datasets_of_a_portal_never_list_another_portals_or_organizations_records() {
    let state = world();
    let page = http(&state, person("bbadmin"), "GET", &datasets_route("bbsk-skoly"), None).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["total"], 2);
    assert_eq!(
        page.body["items"],
        json!([
            { "name": "bbsk-skoly-air", "title": "Air", "source": "kosice", "hidden": false },
            { "name": "bbsk-skoly-hidden", "title": "Hidden", "source": "endpoints", "hidden": true },
        ])
    );
    assert!(!page.body.to_string().contains("praha"), "{}", page.body);
    let second = http(
        &state,
        person("bbadmin"),
        "GET",
        &format!("{}?offset=1&limit=1", datasets_route("bbsk-skoly")),
        None,
    )
    .await;
    assert_eq!(second.body["items"][0]["name"], "bbsk-skoly-hidden");
    for bad in ["limit=0", "limit=201", "limit=x", "other=1"] {
        let r = http(
            &state,
            person("bbadmin"),
            "GET",
            &format!("{}?{bad}", datasets_route("bbsk-skoly")),
            None,
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
}
