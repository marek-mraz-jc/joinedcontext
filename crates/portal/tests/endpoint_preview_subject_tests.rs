//! The filter preview names the caller to the broker exactly as the gateway's read does for the
//! same person and Endpoint (EP-14, EP-16, ADR-N-067 2.3, AP-97): each test builds what the
//! gateway's hop carries with the gateway's own rules (context-gateway `auth::subject` and
//! `tenancy::caller_headers`, the headers `pin_caller` writes) and compares the preview's request
//! to the broker with it. Equal headers are one decision: the broker decides on them alone.

mod common;

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, session_cookie, CSRF};
use context_gateway::auth::subject::{anonymous_with, person as gateway_person, with_endpoint_roles};
use context_gateway::auth::token::Claims;
use context_gateway::middleware::tenancy::caller_headers;
use context_gateway::resolver::EndpointRoles;
use jc_core::kinds::EndpointSpec;
use joinedcontext_portal::auth::csrf::CSRF_HEADER;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::reconciler::views::ViewSync;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const TOKEN: &str = "an-admin-token-of-at-least-32-bytes!";

/// The Endpoints of the space `air` of `city`: an organization one granting `analyst` to jana by
/// Keycloak id, a public one, an App's public one with its caller role, and an App's organization
/// one whose `editor` only the App's own client can carry.
fn endpoints() -> Vec<(&'static str, Value)> {
    let spec = |slug: &str, audience: &str, extra: Value| {
        let mut spec = json!({ "contextSpaceRef": "air", "slug": slug, "audience": audience,
                               "enabledRepresentations": ["ngsi-ld"] });
        spec.as_object_mut()
            .expect("an object")
            .extend(extra.as_object().cloned().unwrap_or_default());
        spec
    };
    vec![
        (
            "air-staff",
            spec(
                "w3nq7pzk2mtv6hx5brj4gcdsfa",
                "organization",
                json!({"roles": [{"name": "analyst", "subjects": [{"user": "sub-jana"}]}]}),
            ),
        ),
        (
            "air-public",
            spec("k4y7pq2mzt6vhx3nbwrs5cjdaf", "public", json!({})),
        ),
        (
            "app-map",
            spec(
                "ujzdegxdncf32epf3dhodzdoci",
                "public",
                json!({"callerRole": true}),
            ),
        ),
        (
            "app-tool",
            spec(
                "s2jhtlgmxgedn73u55xtplpft7",
                "organization",
                json!({"callerRole": true,
                       "roles": [{"name": "editor", "subjects": [{"user": "sub-jana"}]}]}),
            ),
        ),
    ]
}

fn repository(dir: &Path) -> jcctl::loader::Repository {
    let write = |rel: &str, kind: &str, name: &str, spec: Value| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        let manifest = json!({ "apiVersion": "joinedcontext.com/v1alpha1", "kind": kind,
                               "metadata": { "name": name, "namespace": "city" }, "spec": spec });
        std::fs::write(path, manifest.to_string()).expect("write");
    };
    write(
        "projects/city/spaces/air/space.yaml",
        "ContextSpace",
        "air",
        json!({"isSandbox": false}),
    );
    for (name, spec) in endpoints() {
        write(
            &format!("projects/city/spaces/air/endpoints/{name}.yaml"),
            "Endpoint",
            name,
            spec,
        );
    }
    for app in ["map", "tool"] {
        write(
            &format!("projects/city/apps/{app}/app.yaml"),
            "App",
            app,
            json!({"kind": "static", "source": {"path": "./src"}, "build": {}}),
        );
    }
    write(
        "projects/city/spaces/air/policies/cars.yaml",
        "Policy",
        "cars",
        json!({"contextSpaceRef": "air", "assigner": "did:web:city.example",
               "assignee": {"kind": "role", "id": "public"}, "operations": ["retrieveOps"],
               "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}),
    );
    // Each App's own grant, to its Endpoint's caller role (AP-139).
    for app in ["app-map", "app-tool"] {
        write(
            &format!("projects/city/spaces/air/policies/{app}.yaml"),
            "Policy",
            app,
            json!({"contextSpaceRef": "air", "assigner": "did:web:city.example",
                   "assignee": {"kind": "role", "id": format!("endpoint:city/{app}")},
                   "operations": ["retrieveOps"],
                   "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}),
        );
    }
    jcctl::loader::Repository::load(dir).expect("the repository loads")
}

async fn broker() -> MockServer {
    let broker = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/q/views"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&broker)
        .await;
    Mock::given(method("POST"))
        .and(path_regex("^/q/views/[a-z0-9-]+/preview$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"count": 0, "entities": []})))
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

/// jana and ola read the space `air`; `city` is a project of the Organization `hel`.
async fn rig() -> Rig {
    let broker = broker().await;
    let mut state = AppState::new(Config::for_tests(), None);
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", "city", json!({})));
    for (name, spec) in endpoints() {
        state
            .mirror
            .upsert(envelope("Endpoint", name, "city", spec));
    }
    state.mirror.upsert(envelope(
        "Project",
        "city",
        ORG_NAMESPACE,
        json!({ "organizationRef": "hel" }),
    ));
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
        json!({ "subjects": [{ "user": "jana@hel.fi" }, { "user": "ola@hel.fi" }],
                "role": "space-reader", "scope": { "contextSpace": "air" } }),
    ));
    let dir = std::env::temp_dir().join(format!(
        "portal-preview-subject-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    let token = dir.join("token");
    let repo = repository(&dir);
    std::fs::write(&token, format!("{TOKEN}\n")).expect("write the token");
    let views = ViewSync::new(broker.uri(), &token);
    views.converge(&repo).await;
    state.views = Some(Arc::new(views));
    Rig { state, broker }
}

/// The ID token the realm gives `who` on `portal-ui`: their Keycloak id, groups, Organization
/// standing and realm roles (realm `jc-portal.json`: `jc_org` and `jc_groups` are ID-token
/// claims on `portal-ui`). The Portal verified it at sign-in; the test signs nothing.
fn id_token(claims: &Value) -> String {
    use base64::Engine;
    let part = |v: &Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
    };
    format!("{}.{}.sig", part(&json!({"alg": "none"})), part(claims))
}

fn jana_claims() -> Value {
    json!({ "sub": "sub-jana", "iss": "https://kc.example/realms/jc", "exp": 4_102_444_800_i64,
            "azp": "portal-ui", "preferred_username": "jana", "groups": ["/city"],
            "jc_org": ["hel"], "jc_groups": ["/hel/team", "/esp/team"],
            "realm_access": { "roles": ["viewer"] } })
}

fn ola_claims() -> Value {
    json!({ "sub": "sub-ola", "iss": "https://kc.example/realms/jc", "exp": 4_102_444_800_i64,
            "azp": "portal-ui", "preferred_username": "ola" })
}

/// The Portal session of the person `claims` name, the way sign-in makes it.
fn session_of(claims: &Value) -> Session {
    let name = claims["preferred_username"].as_str().expect("a name");
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| s.trim_start_matches('/').to_owned())
            .collect()
    };
    let now = session::now_unix();
    Session {
        identity: Identity {
            subject: claims["sub"].as_str().expect("a sub").to_owned(),
            roles: strings(&claims["realm_access"]["roles"]),
            groups: strings(&claims["groups"]),
            ..person(name)
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: id_token(claims),
        access_expires_at: now + 3600,
        refresh_token: None,
    }
}

/// The preview of `endpoint` as `session`, its claims kept on the state when `kept`, with
/// `forged` headers a browser added; the `x-jc-*` headers the broker received, sorted.
async fn preview(
    r: &Rig,
    session: &Session,
    kept: bool,
    endpoint: &str,
    forged: &[(&str, &str)],
) -> (StatusCode, Option<Vec<(String, String)>>) {
    if kept {
        r.state.keep_session_claims(session);
    }
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/projects/city/endpoints/{endpoint}/preview"))
        .header(header::COOKIE, session_cookie(&r.state.config, session))
        .header(CSRF_HEADER, CSRF)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in forged {
        request = request.header(*name, *value);
    }
    let response = server::app(r.state.clone())
        .oneshot(
            request
                .body(Body::from(json!({"type": "Car"}).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = response.into_body().collect().await.expect("a body").to_bytes();
    if status != StatusCode::OK {
        eprintln!("{status}: {}", String::from_utf8_lossy(&body));
    }
    let asked = r
        .broker
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .rfind(|request| request.url.path().ends_with("/preview"))
        .map(|request| {
            let mut headers: Vec<(String, String)> = request
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
            headers.sort();
            headers
        });
    (status, asked)
}

/// What the gateway's hop to the broker names about the caller of `claims` (or the anonymous
/// one) on `endpoint`: its subject built as `subject_of` and `anonymous_on` build it, written out
/// as `pin_caller` writes it.
fn gateway(claims: Option<&Value>, endpoint: &str) -> Vec<(String, String)> {
    let spec_json = endpoints()
        .into_iter()
        .find(|(name, _)| *name == endpoint)
        .map(|(_, spec)| spec)
        .expect("a known Endpoint");
    let spec: EndpointSpec = serde_json::from_value(spec_json).expect("an Endpoint spec");
    let roles = match endpoint.strip_prefix("app-") {
        Some(_) => EndpointRoles::of_app("city", endpoint, &spec, endpoint.to_owned()),
        None => EndpointRoles::of("city", endpoint, &spec),
    };
    let subject = match claims {
        None => anonymous_with(&roles),
        Some(claims) => {
            let claims: Claims = serde_json::from_value(claims.clone()).expect("claims");
            let mut subject =
                gateway_person(&claims, spec.audience, Some("hel")).expect("a person");
            with_endpoint_roles(&mut subject, &roles, Some(&claims));
            subject
        }
    };
    let mut headers: Vec<(String, String)> = caller_headers(&subject)
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    headers.sort();
    headers
}

fn value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// A `user` assignee and an endpoint role naming jana by Keycloak id. The gateway names her
/// `sub-jana` and gives her `endpoint:city/air-staff/analyst`, so the broker admits her; the
/// preview named her `jana` and dropped the role, so the broker refused her.
#[tokio::test]
async fn a_user_assignee_by_keycloak_id_reads_the_preview_as_on_the_gateway() {
    let r = rig().await;
    let (status, asked) = preview(&r, &session_of(&jana_claims()), true, "air-staff", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let asked = asked.expect("the broker was asked");
    assert_eq!(value(&asked, "x-jc-subject"), Some("sub-jana"), "{asked:?}");
    assert!(
        value(&asked, "x-jc-roles").is_some_and(|r| r.contains("air-staff%2Fanalyst")),
        "{asked:?}"
    );
    assert_eq!(asked, gateway(Some(&jana_claims()), "air-staff"));
}

/// An `organization` assignee and a `group` one: the gateway says `x-jc-member: true` and the
/// groups of the Endpoint's Organization (`team`, never `/esp/team`); the preview said neither
/// and sent the raw groups.
#[tokio::test]
async fn an_organization_member_is_a_member_to_the_preview_too() {
    let r = rig().await;
    let (_, asked) = preview(&r, &session_of(&jana_claims()), true, "air-staff", &[]).await;
    let asked = asked.expect("the broker was asked");
    assert_eq!(value(&asked, "x-jc-member"), Some("true"), "{asked:?}");
    assert_eq!(value(&asked, "x-jc-groups"), Some("team"), "{asked:?}");
    assert_eq!(asked, gateway(Some(&jana_claims()), "air-staff"));
}

/// EP-16, AP-97: a public Endpoint's preview reads as the anonymous caller, who on the gateway
/// holds `public` and the Endpoint's caller role, the role an App's Policy grants (the smoke row
/// app-banskabystrica-data); the preview sent `public` alone.
#[tokio::test]
async fn a_public_endpoint_reads_as_the_anonymous_caller_with_its_caller_role() {
    let r = rig().await;
    let (status, asked) = preview(&r, &session_of(&jana_claims()), true, "app-map", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let asked = asked.expect("the broker was asked");
    assert_eq!(
        asked,
        vec![(
            "x-jc-roles".to_owned(),
            "endpoint%3Acity%2Fapp-map,public".to_owned()
        )]
    );
    assert_eq!(asked, gateway(None, "app-map"));
    let (_, plain) = preview(&r, &session_of(&jana_claims()), true, "air-public", &[]).await;
    assert_eq!(plain, Some(gateway(None, "air-public")));
}

/// ADR-N-030: an App's own role is the App client's token's; a Portal session's token is
/// `portal-ui`'s, so the gateway gives jana no `editor` and the preview gives none either. Both
/// give her the App Endpoint's caller role.
#[tokio::test]
async fn an_apps_role_is_refused_to_a_portal_session_on_both() {
    let r = rig().await;
    let (_, asked) = preview(&r, &session_of(&jana_claims()), true, "app-tool", &[]).await;
    let asked = asked.expect("the broker was asked");
    let roles = value(&asked, "x-jc-roles").unwrap_or_default().to_owned();
    assert!(!roles.contains("editor"), "{asked:?}");
    assert!(roles.contains("endpoint%3Acity%2Fapp-tool"), "{asked:?}");
    assert_eq!(asked, gateway(Some(&jana_claims()), "app-tool"));
}

/// A person no grant names: named by Keycloak id with an empty `x-jc-roles`, which the broker
/// reads as that person holding no role (never the cluster's own hop, never the anonymous
/// caller), on the preview as on the gateway.
#[tokio::test]
async fn a_person_without_a_grant_is_refused_on_both() {
    let r = rig().await;
    let (_, asked) = preview(&r, &session_of(&ola_claims()), true, "air-staff", &[]).await;
    let asked = asked.expect("the broker was asked");
    assert_eq!(
        asked,
        vec![
            ("x-jc-roles".to_owned(), String::new()),
            ("x-jc-subject".to_owned(), "sub-ola".to_owned()),
        ]
    );
    assert_eq!(asked, gateway(Some(&ola_claims()), "air-staff"));
}

/// AP-97: a realm role shaped as another project's endpoint role is dropped by the gateway; the
/// preview forwarded it, so it held a grant the gateway read does not.
#[tokio::test]
async fn another_projects_endpoint_role_never_travels() {
    let r = rig().await;
    let mut claims = jana_claims();
    claims["realm_access"]["roles"] = json!(["viewer", "endpoint:other/data"]);
    let (_, asked) = preview(&r, &session_of(&claims), true, "air-staff", &[]).await;
    let asked = asked.expect("the broker was asked");
    assert!(
        !value(&asked, "x-jc-roles").unwrap_or_default().contains("other"),
        "{asked:?}"
    );
    assert_eq!(asked, gateway(Some(&claims), "air-staff"));
}

/// GW20: identity headers a browser sends are never forwarded; the broker hears the server's.
#[tokio::test]
async fn browser_sent_identity_headers_never_reach_the_broker() {
    let r = rig().await;
    let forged = [
        ("x-jc-roles", "admin"),
        ("x-jc-subject", "root"),
        ("x-jc-member", "true"),
        ("x-jc-groups", "ops"),
        ("x-jc-client", "context-gateway"),
    ];
    let (_, asked) = preview(&r, &session_of(&ola_claims()), true, "air-staff", &forged).await;
    assert_eq!(asked, Some(gateway(Some(&ola_claims()), "air-staff")));
    let (_, public) = preview(&r, &session_of(&ola_claims()), true, "app-map", &forged).await;
    assert_eq!(public, Some(gateway(None, "app-map")));
}

/// A session whose ID token this process does not hold (a restart, another replica) is named by
/// Keycloak id with no Organization standing: never more than the gateway reads, here less.
#[tokio::test]
async fn without_the_kept_id_token_the_preview_reads_narrower() {
    let r = rig().await;
    let (_, asked) = preview(&r, &session_of(&jana_claims()), false, "air-staff", &[]).await;
    let asked = asked.expect("the broker was asked");
    assert_eq!(value(&asked, "x-jc-subject"), Some("sub-jana"), "{asked:?}");
    assert_eq!(value(&asked, "x-jc-member"), None, "{asked:?}");
    assert_eq!(value(&asked, "x-jc-groups"), None, "{asked:?}");
    let gateway = gateway(Some(&jana_claims()), "air-staff");
    assert!(
        asked.iter().all(|pair| gateway.contains(pair)),
        "{asked:?} vs {gateway:?}"
    );
}
