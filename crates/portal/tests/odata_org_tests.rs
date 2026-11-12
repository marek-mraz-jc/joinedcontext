//! The OData service per Organization at `/{org}/api/v1/` (ADR-N-070, API/13 §2a, T-6810),
//! through the whole router in both of jc's modes: a bound host (`Host: portal.hel.example`) and
//! a shared host whose first path segment jc's registry read (`RequestOrganization`).
//!
//! - A caller of one Organization lists, counts, `$expand`s and `$filter`s toward the other's
//!   projects and gets none of their rows; their keys answer 404.
//! - Each query option answers or is refused with its status, never ignored.
//! - A `$skiptoken` holds only for its caller and read.
//! - REST compat keeps its body on the same root; both carry `Vary`.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::envelope;
use joinedcontext_portal::auth::csrf::{self, CSRF_COOKIE};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::ClientAuth;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::organization_host::{self, OrganizationHost, RequestOrganization};
use joinedcontext_portal::people::People;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const HEL: &str = "portal.hel.example";
const BA: &str = "portal.ba.example";
/// jc's shared host: the Organization is the path's first segment, which jc removed.
const SHARED: &str = "city.example";
const ROOTS: &str = "platform-admins";

fn host(name: &str) -> OrganizationHost {
    let at = if name == "hel" { HEL } else { BA };
    OrganizationHost {
        organization: name.to_owned(),
        base: format!("https://{at}").parse().expect("url"),
    }
}

fn person(name: &str, groups: &[&str]) -> Identity {
    Identity {
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
        ..common::person(name)
    }
}

/// hel (the installation) holds helsinki with three spaces, ba holds bratislava with two.
fn state() -> AppState {
    let config = Config {
        organization_hosts: vec![host("hel"), host("ba")],
        bootstrap_admins: ROOTS.to_owned(),
        ..Config::for_tests()
    };
    let state = AppState::new(config, None);
    let m = &state.mirror;
    m.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "ContextSpace", "Endpoint", "Workspace"], "verbs": ["read"] }] }),
    ));
    m.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    m.upsert(envelope("Organization", "ba", ORG_NAMESPACE, json!({})));
    for (project, owner, spaces) in [
        ("helsinki", "hel", &["air", "noise", "water"][..]),
        ("bratislava", "ba", &["air", "traffic"][..]),
    ] {
        m.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({ "organizationRef": owner }),
        ));
        for space in spaces {
            m.upsert(envelope("ContextSpace", space, project, json!({})));
        }
    }
    m.upsert(envelope(
        "Role",
        "editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "ContextSpace", "Endpoint", "Workspace"], "verbs": ["read", "propose"] }] }),
    ));
    m.upsert(envelope(
        "RoleBinding",
        "hel-editor-hel",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "hel-editor@hel.fi" }], "role": "editor",
                "scope": { "organization": "hel" } }),
    ));
    for (who, org) in [
        ("hel-only", "hel"),
        ("hel-two", "hel"),
        ("ba-only", "ba"),
        ("both", "hel"),
        ("both", "ba"),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{org}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": "reader",
                    "scope": { "organization": org } }),
        ));
    }
    state
}

/// How the request reached the Portal: on an Organization's bound host, or on jc's shared host
/// with the Organization's path segment already read off by jc's registry.
#[derive(Clone, Copy, Debug)]
enum Mode {
    Host(&'static str),
    Path(&'static str),
}

impl Mode {
    fn org(self) -> &'static str {
        match self {
            Self::Host(org) | Self::Path(org) => org,
        }
    }
}

/// The session cookies minted on `org`'s host, or on the deployment's own address for "".
async fn cookies(config: &Config, identity: Identity, org: &str) -> String {
    let mint = async {
        let now = session::now_unix();
        let session = Session {
            identity,
            expires_at: now + 3600,
            issued_at: now,
            signed_in_at: None,
            id_token: "id".into(),
            access_expires_at: now + 3600,
            refresh_token: None,
        };
        let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
            .expect("store");
        let token = csrf::Token::mint();
        let response = (jar, StatusCode::OK).into_response();
        let mut parts: Vec<String> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
            .collect();
        parts.push(format!("{CSRF_COOKIE}={}", token.as_str()));
        parts.join("; ")
    };
    if org.is_empty() {
        mint.await
    } else {
        organization_host::scope(host(org), mint).await
    }
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

async fn send(
    state: &AppState,
    mode: Mode,
    cookie: &str,
    uri: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, cookie);
    request = match mode {
        Mode::Host(org) => request.header(header::HOST, if org == "hel" { HEL } else { BA }),
        Mode::Path(_) => request.header(header::HOST, SHARED),
    };
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let mut request = request.body(Body::empty()).expect("request");
    if let Mode::Path(org) = mode {
        request
            .extensions_mut()
            .insert(RequestOrganization(org.to_owned()));
    }
    let response = server::app(state.clone())
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    Answer {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn get(state: &AppState, mode: Mode, who: &str, uri: &str) -> Answer {
    let cookie = cookies(&state.config, person(who, groups_of(who)), mode.org()).await;
    send(state, mode, &cookie, uri, &[]).await
}

fn groups_of(who: &str) -> &'static [&'static str] {
    if who == "root" {
        &[ROOTS]
    } else {
        &[]
    }
}

fn names(body: &Value, member: &str) -> Vec<String> {
    body["value"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r[member].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

const MODES: [Mode; 4] = [
    Mode::Host("hel"),
    Mode::Path("hel"),
    Mode::Host("ba"),
    Mode::Path("ba"),
];

/// Red first: a caller of one Organization reaches nothing of the other, however they ask.
#[tokio::test]
async fn an_organization_lists_counts_expands_and_filters_only_its_own_projects() {
    let state = state();
    for mode in MODES {
        let (own, other, own_spaces) = if mode.org() == "hel" {
            ("helsinki", "bratislava", 3)
        } else {
            ("bratislava", "helsinki", 2)
        };
        let callers: &[&str] = if mode.org() == "hel" {
            &["hel-only", "both", "root"]
        } else {
            &["ba-only", "both", "root"]
        };
        for who in callers {
            let at = format!("{who} {mode:?}");
            let projects = get(&state, mode, who, "/api/v1/projects?$count=true").await;
            assert_eq!(projects.status, StatusCode::OK, "{at}: {}", projects.text);
            let body = projects.json();
            assert_eq!(names(&body, "name"), vec![own.to_owned()], "{at}: {body}");
            assert_eq!(body["@odata.count"], json!(1), "{at}");

            let spaces = get(&state, mode, who, "/api/v1/spaces?$count=true").await;
            let body = spaces.json();
            assert_eq!(spaces.status, StatusCode::OK, "{at}: {body}");
            assert_eq!(body["@odata.count"], json!(own_spaces), "{at}: {body}");
            assert!(
                names(&body, "project").iter().all(|p| p == own),
                "{at}: {body}"
            );
            let count = get(&state, mode, who, "/api/v1/spaces/$count").await;
            assert_eq!(count.text, own_spaces.to_string(), "{at}");

            let probe = format!("/api/v1/spaces?$filter=project%20eq%20'{other}'&$count=true");
            let body = get(&state, mode, who, &probe).await.json();
            assert_eq!(body["@odata.count"], json!(0), "{at}: {body}");

            let expanded = get(&state, mode, who, "/api/v1/projects?$expand=spaces").await;
            let body = expanded.json();
            assert_eq!(expanded.status, StatusCode::OK, "{at}: {body}");
            let rows = body["value"].as_array().cloned().unwrap_or_default();
            assert_eq!(rows.len(), 1, "{at}: {body}");
            assert_eq!(
                rows[0]["spaces"].as_array().map(Vec::len),
                Some(own_spaces),
                "{at}: {body}"
            );

            for uri in [
                format!("/api/v1/projects('{other}')"),
                format!("/api/v1/projects/{other}/spaces?$top=5"),
                format!("/api/v1/projects('{other}')/spaces/$count"),
                format!("/api/v1/projects('{other}')?$expand=spaces"),
                format!("/api/v1/spaces(project='{other}',name='air')"),
            ] {
                let refused = get(&state, mode, who, &uri).await;
                assert_eq!(
                    refused.status,
                    StatusCode::NOT_FOUND,
                    "{at} {uri}: {}",
                    refused.text
                );
                assert_eq!(
                    refused.json()["error"]["type"],
                    json!("https://joinedcontext.com/errors/resource-not-found"),
                    "{at} {uri}"
                );
            }
            // The control: the own project by every path.
            for uri in [
                format!("/api/v1/projects('{own}')"),
                format!("/api/v1/projects/{own}/spaces?$top=5"),
                format!("/api/v1/projects('{own}')/spaces('air')"),
                format!("/api/v1/spaces(project='{own}',name='air')"),
            ] {
                let read = get(&state, mode, who, &uri).await;
                assert_eq!(read.status, StatusCode::OK, "{at} {uri}: {}", read.text);
            }
        }
        // A person of the other Organization alone is refused this one's service outright.
        let stranger = if mode.org() == "hel" {
            "ba-only"
        } else {
            "hel-only"
        };
        let refused = get(&state, mode, stranger, "/api/v1/spaces").await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{mode:?}: {}",
            refused.text
        );
    }
}

/// One table over the organization-wide `spaces` set: each option answers what it says.
#[tokio::test]
async fn each_query_option_answers_on_an_organization_wide_set() {
    let state = state();
    let mode = Mode::Host("hel");
    let cases: [(&str, Vec<&str>, Option<u64>); 7] = [
        ("$filter=startswith(name,'n')", vec!["noise"], None),
        ("$orderby=name%20desc", vec!["water", "noise", "air"], None),
        ("$orderby=name&$top=2", vec!["air", "noise"], None),
        ("$orderby=name&$skip=2", vec!["water"], None),
        (
            "$filter=name%20ne%20'air'&$count=true&$orderby=name",
            vec!["noise", "water"],
            Some(2),
        ),
        (
            "$filter=name%20in%20('air','water')&$orderby=name",
            vec!["air", "water"],
            None,
        ),
        (
            "$select=name&$orderby=name",
            vec!["air", "noise", "water"],
            None,
        ),
    ];
    for (query, want, count) in cases {
        let answer = get(&state, mode, "hel-only", &format!("/api/v1/spaces?{query}")).await;
        let body = answer.json();
        assert_eq!(answer.status, StatusCode::OK, "{query}: {body}");
        assert_eq!(names(&body, "name"), want, "{query}: {body}");
        if let Some(count) = count {
            assert_eq!(body["@odata.count"], json!(count), "{query}");
        }
        assert!(
            body["value"][0].get("_key").is_none(),
            "{query}: the evaluator's key leaked: {body}"
        );
    }
    let selected = get(&state, mode, "hel-only", "/api/v1/spaces?$select=name")
        .await
        .json();
    assert_eq!(
        selected["value"][0]
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["@odata.etag".to_owned(), "name".to_owned()]),
        "{selected}"
    );
    assert!(
        selected["@odata.context"].as_str().is_some_and(
            |c| c.starts_with("https://portal.hel.example/api/v1/$metadata#spaces(name)")
        )
    );
}

/// Server-driven paging: the nextLink's `$skiptoken` answers the next page for its caller only,
/// and a token changed by a byte or carried to another caller or read is refused.
#[tokio::test]
async fn the_next_link_pages_on_for_its_own_caller_and_read_only() {
    let state = state();
    let mode = Mode::Host("hel");
    let ari = cookies(&state.config, person("hel-only", &[]), "hel").await;
    let bea = cookies(&state.config, person("hel-two", &[]), "hel").await;
    let paged = [("prefer", "odata.maxpagesize=2")];
    let first = send(&state, mode, &ari, "/api/v1/spaces?$orderby=name", &paged).await;
    let body = first.json();
    assert_eq!(names(&body, "name"), ["air", "noise"], "{body}");
    assert_eq!(
        first
            .headers
            .get("preference-applied")
            .and_then(|v| v.to_str().ok()),
        Some("odata.maxpagesize=2")
    );
    let next = body["@odata.nextLink"]
        .as_str()
        .expect("nextLink")
        .to_owned();
    let path = next
        .strip_prefix("https://portal.hel.example")
        .expect("the registry's host, never the request's")
        .to_owned();
    let second = send(&state, mode, &ari, &path, &paged).await.json();
    assert_eq!(names(&second, "name"), ["water"], "{second}");
    assert!(second.get("@odata.nextLink").is_none(), "{second}");

    let other_caller = send(&state, mode, &bea, &path, &paged).await;
    assert_eq!(
        other_caller.status,
        StatusCode::BAD_REQUEST,
        "{}",
        other_caller.text
    );
    let other_read = path.replace("$orderby=name", "$orderby=name%20desc");
    let moved = send(&state, mode, &ari, &other_read, &paged).await;
    assert_eq!(moved.status, StatusCode::BAD_REQUEST, "{}", moved.text);
    let tampered = path.replacen("skiptoken=", "skiptoken=9", 1);
    let tampered = send(&state, mode, &ari, &tampered, &paged).await;
    assert_eq!(
        tampered.status,
        StatusCode::BAD_REQUEST,
        "{}",
        tampered.text
    );
    // The Host a client forges never reaches the link.
    let forged = send(
        &state,
        mode,
        &ari,
        "/api/v1/spaces?$orderby=name",
        &[
            ("prefer", "odata.maxpagesize=1"),
            ("x-forwarded-host", "evil.example"),
        ],
    )
    .await
    .json();
    assert!(
        forged["@odata.nextLink"]
            .as_str()
            .is_some_and(|l| l.starts_with("https://portal.hel.example/")),
        "{forged}"
    );
}

/// A request, its extra headers and the status it must answer.
type Case = (
    &'static str,
    &'static [(&'static str, &'static str)],
    StatusCode,
);

/// What the service does not serve answers its status and the OData error object; nothing is
/// silently ignored.
#[tokio::test]
async fn each_refused_option_answers_its_status() {
    let state = state();
    let mode = Mode::Path("hel");
    let cases: [Case; 15] = [
        (
            "/api/v1/spaces?$search=air",
            &[],
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/api/v1/spaces?$apply=groupby((name))",
            &[],
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/api/v1/spaces?$compute=name%20as%20n",
            &[],
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/api/v1/spaces?$deltatoken=1",
            &[],
            StatusCode::NOT_IMPLEMENTED,
        ),
        ("/api/v1/$batch", &[], StatusCode::NOT_IMPLEMENTED),
        (
            "/api/v1/projects?$expand=spaces($levels=2)",
            &[],
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/api/v1/projects?$expand=spaces($expand=x)",
            &[],
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/projects?$expand=title",
            &[],
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/spaces?$expand=spaces",
            &[],
            StatusCode::BAD_REQUEST,
        ),
        ("/api/v1/spaces?$top=1001", &[], StatusCode::BAD_REQUEST),
        (
            "/api/v1/spaces?$filter=secret%20eq%201",
            &[],
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/spaces?$top=1&limit=5",
            &[],
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/spaces?$format=xml",
            &[],
            StatusCode::NOT_ACCEPTABLE,
        ),
        (
            "/api/v1/spaces?$top=1",
            &[("accept", "application/xml")],
            StatusCode::NOT_ACCEPTABLE,
        ),
        ("/api/v1/nothings?$top=1", &[], StatusCode::NOT_FOUND),
    ];
    let cookie = cookies(&state.config, person("hel-only", &[]), "hel").await;
    for (uri, extra, want) in cases {
        let answer = send(&state, mode, &cookie, uri, extra).await;
        assert_eq!(answer.status, want, "{uri}: {}", answer.text);
        let error = &answer.json()["error"];
        assert_eq!(
            error["status"],
            json!(want.as_u16()),
            "{uri}: {}",
            answer.text
        );
        assert!(
            error["type"]
                .as_str()
                .is_some_and(|t| t.starts_with("https://joinedcontext.com/errors/")),
            "{uri}"
        );
        assert!(
            error["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{uri}"
        );
    }
    // A write is not a request this service answers.
    let posted = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects('helsinki')")
                .header(header::HOST, HEL)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(posted.status(), StatusCode::METHOD_NOT_ALLOWED);
    // No session: 401, as the OData error object.
    let anonymous = send(&state, mode, "", "/api/v1/spaces?$top=1", &[]).await;
    assert_eq!(
        anonymous.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        anonymous.text
    );
    assert_eq!(anonymous.json()["error"]["status"], json!(401));
}

/// `$metadata` in XML and JSON, the service document, and REST compat beside them on one root.
#[tokio::test]
async fn metadata_service_document_and_rest_compat_share_the_root() {
    let state = state();
    for mode in [Mode::Host("hel"), Mode::Path("ba")] {
        let xml = get(&state, mode, "both", "/api/v1/$metadata").await;
        assert_eq!(xml.status, StatusCode::OK, "{mode:?}: {}", xml.text);
        assert!(xml.text.starts_with("<?xml"), "{mode:?}");
        for needle in [
            r#"<EntitySet Name="projects""#,
            r#"<NavigationPropertyBinding Path="spaces" Target="spaces"/>"#,
            r#"<PropertyRef Name="project"/>"#,
            r#"<Singleton Name="me""#,
        ] {
            assert!(xml.text.contains(needle), "{mode:?}: no {needle}");
        }
        let json = get(&state, mode, "both", "/api/v1/$metadata?$format=json").await;
        assert_eq!(json.status, StatusCode::OK, "{}", json.text);
        assert!(
            json.json()["JoinedContext.Portal"]["Service"]["me"].is_object(),
            "{}",
            json.text
        );

        let doc = get(&state, mode, "both", "/api/v1/").await.json();
        assert!(
            names(&doc, "name").contains(&"projects".to_owned()),
            "{doc}"
        );

        let me = get(&state, mode, "both", "/api/v1/me?$format=json")
            .await
            .json();
        assert_eq!(me["username"], json!("both"), "{me}");
        let org = get(&state, mode, "both", "/api/v1/Organization?$format=json")
            .await
            .json();
        assert_eq!(org["name"], json!(mode.org()), "{org}");
    }
    // REST compat: the same path without an OData sign answers today's body.
    let mode = Mode::Host("hel");
    let rest = get(&state, mode, "hel-only", "/api/v1/projects").await;
    assert_eq!(rest.json()["kind"], json!("List"), "{}", rest.text);
    assert!(rest
        .headers
        .get_all(header::VARY)
        .iter()
        .any(|v| v.to_str().is_ok_and(|v| v.contains("OData-Version"))));
    let cookie = cookies(&state.config, person("hel-only", &[]), "hel").await;
    let odata = send(
        &state,
        mode,
        &cookie,
        "/api/v1/projects",
        &[("odata-version", "4.0")],
    )
    .await;
    assert_eq!(names(&odata.json(), "name"), ["helsinki"], "{}", odata.text);
    assert_eq!(
        odata
            .headers
            .get("odata-version")
            .and_then(|v| v.to_str().ok()),
        Some("4.01")
    );
    // The per-project service stays where it was.
    let alias = get(
        &state,
        mode,
        "hel-only",
        "/api/v1/projects/helsinki/odata/spaces?$top=1",
    )
    .await;
    assert_eq!(alias.status, StatusCode::OK, "{}", alias.text);
}

/// The deployment's own address of a jc with several Organizations names none: the service
/// there answers 404 and points at `/{org}/api/v1/`, it never picks one.
#[tokio::test]
async fn an_unpinned_request_with_several_organizations_reads_none() {
    let state = state();
    let cookie = cookies(&state.config, person("root", &[ROOTS]), "").await;
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects?$top=5")
                .header(header::HOST, SHARED)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A write as an action: POST with the session's CSRF token, `If-Match` when given.
/// The registry's functions (API/13 §2b, T-6815): each answers 401 to nobody and 404 on a project
/// of another Organization; one the caller's `$metadata` leaves out is refused them; a function
/// answers what MCP `tools/call` answers for the same input.
#[tokio::test]
async fn every_function_is_refused_where_metadata_leaves_it_out() {
    use joinedcontext_portal::api::odata_ops::{form, Form};
    let state = state();
    let mode = Mode::Host("hel");
    let viewer = get(&state, mode, "hel-only", "/api/v1/$metadata").await;
    assert_eq!(viewer.status, StatusCode::OK, "{}", viewer.text);
    let functions: Vec<&str> = joinedcontext_portal::ops::registry()
        .iter()
        .map(|op| op.name)
        .filter(|name| form(name) == Some(Form::Function))
        .collect();
    assert!(functions.len() >= 31, "{functions:?}");
    assert!(
        viewer
            .text
            .contains(r#"<Function Name="jc_schema_get" IsBound="true">"#),
        "{}",
        viewer.text
    );
    for name in &functions {
        let uri = format!("/api/v1/projects('helsinki')/JoinedContext.Portal.{name}()");
        let nobody = send(&state, mode, "", &uri, &[]).await;
        assert_eq!(
            nobody.status,
            StatusCode::UNAUTHORIZED,
            "{name}: {}",
            nobody.text
        );
        // A member of none of this Organization's projects stops at its session gate.
        let elsewhere = get(&state, mode, "ba-only", &uri).await;
        assert_eq!(
            elsewhere.status,
            StatusCode::FORBIDDEN,
            "{name}: {}",
            elsewhere.text
        );
        let other_org = get(
            &state,
            mode,
            "hel-only",
            &format!("/api/v1/projects('bratislava')/JoinedContext.Portal.{name}()"),
        )
        .await;
        assert_eq!(
            other_org.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            other_org.text
        );
        assert!(
            other_org.json()["error"]["code"].is_string(),
            "{}",
            other_org.text
        );
        if !viewer.text.contains(&format!(r#"<Function Name="{name}""#)) {
            let refused = get(&state, mode, "hel-only", &uri).await;
            assert!(
                matches!(
                    refused.status,
                    StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
                ),
                "{name} is not in the viewer's $metadata yet answered {}: {}",
                refused.status,
                refused.text
            );
        }
    }
}

#[tokio::test]
async fn a_function_answers_as_mcp_tools_call_and_takes_aliases() {
    use joinedcontext_portal::ops::{Caller, Via};
    let state = state();
    let mode = Mode::Host("hel");
    let odata = get(
        &state,
        mode,
        "hel-only",
        "/api/v1/projects/helsinki/JoinedContext.Portal.jc_schema_get(kind=@k)?@k='ContextSpace'",
    )
    .await;
    assert_eq!(odata.status, StatusCode::OK, "{}", odata.text);
    let body = odata.json();
    assert!(
        body["@odata.context"]
            .as_str()
            .is_some_and(|c| c.ends_with("$metadata#Edm.Untyped")),
        "{body}"
    );
    let caller = Caller::new(person("hel-only", &[]), Via::Mcp);
    let mcp = common::doors::mcp_rpc(
        &caller,
        &state,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "jc_schema_get",
            "arguments": { "project": "helsinki", "kind": "ContextSpace" } } }),
    )
    .await;
    assert_eq!(body["value"], mcp["result"]["structuredContent"], "{mcp}");

    for (uri, status) in [
        // A literal OData does not know, a parameter the operation does not take.
        (
            "projects('helsinki')/JoinedContext.Portal.jc_schema_get(kind=ContextSpace)",
            400,
        ),
        (
            "projects('helsinki')/JoinedContext.Portal.jc_schema_get(kind='ContextSpace',size=1)",
            422,
        ),
        (
            "projects('helsinki')/JoinedContext.Portal.jc_schema_get(kind=@k)",
            400,
        ),
        (
            "projects('helsinki')/JoinedContext.Portal.jc_nothing()",
            404,
        ),
        // An action is no function, an entity no binding.
        (
            "projects('helsinki')/JoinedContext.Portal.jc_run_create()",
            404,
        ),
        (
            "spaces(project='helsinki',name='air')/JoinedContext.Portal.jc_schema_get(kind='x')",
            404,
        ),
    ] {
        let answer = get(&state, mode, "hel-only", &format!("/api/v1/{uri}")).await;
        assert_eq!(answer.status.as_u16(), status, "{uri}: {}", answer.text);
        assert!(
            answer.json()["error"]["message"].is_string(),
            "{uri}: {}",
            answer.text
        );
    }
}

/// The org-admin's CSDL JSON the SDK's OData client is generated from (T-6816): a change of the
/// service that moves it fails here until the snapshot is written again
/// (`JC_WRITE_SNAPSHOT=1 cargo test --test odata_org_tests sdk_metadata`) and the client
/// regenerated (`node sdk/scripts/odata-gen.mjs`), which CI checks has no diff.
#[tokio::test]
async fn sdk_metadata_snapshot_is_the_org_admins_metadata() {
    // One snapshot per role (T-6821): the SDK client is generated from the org-admin's.
    let state = state();
    let mut stale = Vec::new();
    for (role, who) in [
        ("org-admin", "root"),
        ("editor", "hel-editor"),
        ("viewer", "hel-only"),
        ("anonymous", ""),
    ] {
        let snapshot = format!("sdk/odata/{role}.metadata.json");
        let cookie = if who.is_empty() {
            String::new()
        } else {
            cookies(&state.config, person(who, groups_of(who)), "hel").await
        };
        let answer = send(
            &state,
            Mode::Host("hel"),
            &cookie,
            "/api/v1/$metadata?$format=json",
            &[],
        )
        .await;
        let expected = if who.is_empty() {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::OK
        };
        assert_eq!(answer.status, expected, "{role}: {}", answer.text);
        let pretty = format!(
            "{}\n",
            serde_json::to_string_pretty(&answer.json()).expect("JSON")
        );
        if std::env::var_os("JC_WRITE_SNAPSHOT").is_some() {
            std::fs::write(&snapshot, &pretty).expect("write the snapshot");
        }
        if std::fs::read_to_string(&snapshot).unwrap_or_default() != pretty {
            stale.push(snapshot);
        }
    }
    assert!(
        stale.is_empty(),
        "{stale:?} are not their role's $metadata: write them with JC_WRITE_SNAPSHOT=1 and \
         regenerate the SDK client with node sdk/scripts/odata-gen.mjs"
    );
}

/// Red first (T-6821): a viewer's `$metadata` named every kind's set, the ones whose reads
/// answer 404 included. Now a set is in a caller's `$metadata` exactly when they read it in a
/// project of theirs, and a bound action names no type the caller cannot see.
#[tokio::test]
async fn metadata_names_only_the_sets_the_caller_reads() {
    let state = state();
    let mode = Mode::Host("hel");
    let sets: Vec<String> = joinedcontext_portal::resource::kinds()
        .filter(|info| !matches!(info.scope, jc_core::Scope::Organization))
        .map(|info| info.plural.to_owned())
        .chain(["changes".to_owned(), "workspaces".to_owned()])
        .collect();
    let mut wrong = Vec::new();
    for who in ["hel-only", "hel-editor"] {
        let doc = get(&state, mode, who, "/api/v1/$metadata?$format=json")
            .await
            .json();
        let container = &doc["JoinedContext.Portal"]["Service"];
        let xml = get(&state, mode, who, "/api/v1/$metadata").await.text;
        for set in &sets {
            let listed = container.get(set).is_some();
            let read = get(
                &state,
                mode,
                who,
                &format!("/api/v1/projects('helsinki')/{set}"),
            )
            .await;
            // A refusal is 401, 403 or 404; anything else (a 503 of the test's absent change
            // store) is a read the caller was let in to.
            let refused = matches!(read.status.as_u16(), 401 | 403 | 404);
            if listed == refused {
                wrong.push(format!(
                    "{who} {set}: listed {listed}, read {}",
                    read.status
                ));
            }
            let in_xml = xml.contains(&format!("<EntitySet Name=\"{set}\""));
            if listed != in_xml {
                wrong.push(format!("{who} {set}: JSON {listed}, XML {in_xml}"));
            }
        }
        assert!(container.get("projects").is_some(), "{who}: {container}");
        // Every type an action is bound to is a type of this caller's model.
        for (name, member) in doc["JoinedContext.Portal"]
            .as_object()
            .into_iter()
            .flatten()
        {
            for overload in member.as_array().into_iter().flatten() {
                if let Some(ty) = overload["$Parameter"][0]["$Type"].as_str() {
                    let short = ty.trim_start_matches("JoinedContext.Portal.");
                    if doc["JoinedContext.Portal"].get(short).is_none() {
                        wrong.push(format!("{who}: {name} is bound to the hidden {ty}"));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    // The viewer reads four kinds, not every one: the model is theirs, not the org-admin's.
    let viewer = get(&state, mode, "hel-only", "/api/v1/$metadata?$format=json")
        .await
        .json();
    let admin = get(&state, mode, "root", "/api/v1/$metadata?$format=json")
        .await
        .json();
    let count = |doc: &Value| {
        doc["JoinedContext.Portal"]["Service"]
            .as_object()
            .map_or(0, serde_json::Map::len)
    };
    assert!(count(&viewer) < count(&admin), "{viewer}");
    assert!(viewer["JoinedContext.Portal"]["Service"]
        .get("roles")
        .is_none());
    assert!(viewer["JoinedContext.Portal"].get("Role").is_none());
}

/// The names a REST list answers, whatever its envelope.
fn rest_names(body: &Value) -> Vec<String> {
    let rows = body
        .get("items")
        .or_else(|| body.get("value"))
        .unwrap_or(body)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut names: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.pointer("/metadata/name")
                .or_else(|| r.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// PF-56 parity (T-6817): every role reads every project kind over OData as it reads it over REST:
/// the same refusal status, or the same rows; and over MCP `jc_resource_list` the same rows.
#[tokio::test]
async fn every_role_reads_every_set_as_rest_and_mcp_answer_it() {
    use joinedcontext_portal::ops::{Caller, Via};
    let state = state();
    let mode = Mode::Host("hel");
    let mut differ = Vec::new();
    let sets: Vec<(String, String)> = joinedcontext_portal::resource::kinds()
        .filter(|info| !matches!(info.scope, jc_core::Scope::Organization))
        .map(|info| (info.plural.to_owned(), info.kind.to_owned()))
        .collect();
    assert!(sets.len() > 10, "{sets:?}");
    for who in ["", "hel-only", "hel-editor", "root", "ba-only"] {
        let cookie = if who.is_empty() {
            String::new()
        } else {
            cookies(&state.config, person(who, groups_of(who)), "hel").await
        };
        for (plural, kind) in &sets {
            let odata = send(
                &state,
                mode,
                &cookie,
                &format!("/api/v1/projects('helsinki')/{plural}"),
                &[],
            )
            .await;
            let rest = send(
                &state,
                mode,
                &cookie,
                &format!("/api/v1/projects/helsinki/{plural}"),
                &[],
            )
            .await;
            let same = match (odata.status.is_success(), rest.status.is_success()) {
                (true, true) => {
                    let mut o = names(&odata.json(), "name");
                    o.sort();
                    o == rest_names(&rest.json())
                }
                (false, false) => odata.status == rest.status,
                _ => false,
            };
            if !same {
                differ.push(format!(
                    "{who:?} {plural}: OData {} {} / REST {} {}",
                    odata.status,
                    odata.text.chars().take(600).collect::<String>(),
                    rest.status,
                    rest.text.chars().take(160).collect::<String>()
                ));
            }
            // Over MCP a Role list inside a project answers the Organization's Roles too (PF-68);
            // OData and REST key a project's row by its own copy only.
            let both_homes = joinedcontext_portal::resource::by_kind(kind)
                .is_some_and(|info| matches!(info.scope, jc_core::Scope::OrganizationOrProject));
            if who.is_empty() || !odata.status.is_success() || both_homes {
                continue;
            }
            let caller = Caller::new(person(who, groups_of(who)), Via::Mcp);
            let mcp = common::doors::mcp_rpc(
                &caller,
                &state,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                    "name": "jc_resource_list",
                    "arguments": { "project": "helsinki", "kind": kind } } }),
            )
            .await;
            let mut o = names(&odata.json(), "name");
            o.sort();
            let m = rest_names(&mcp["result"]["structuredContent"]);
            if o != m {
                differ.push(format!("{who:?} {plural}: OData {o:?} / MCP {m:?}"));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

/// PF-56 parity of the bound actions (T-6817): each role's dry-run `propose_delete` and
/// `propose_update` answer the status the REST door's dry-run `DELETE` and `PATCH` answer.
#[tokio::test]
async fn every_role_acts_as_the_rest_door_decides() {
    let state = state();
    let etag = state
        .mirror
        .get("helsinki", "ContextSpace", "air")
        .expect("air")
        .etag();
    let entity = "/api/v1/spaces(project='helsinki',name='air')/JoinedContext.Portal.";
    let mut differ = Vec::new();
    for who in ["hel-only", "hel-editor", "root", "ba-only"] {
        let cookie = cookies(&state.config, person(who, groups_of(who)), "hel").await;
        let token = cookie
            .split("; ")
            .find_map(|c| c.strip_prefix(&format!("{CSRF_COOKIE}=")))
            .unwrap_or_default()
            .to_owned();
        for (action, method, body) in [
            ("propose_delete", "DELETE", json!({ "dryRun": true })),
            (
                "propose_update",
                "PATCH",
                json!({ "patch": { "metadata": { "title": "Air" } }, "dryRun": true }),
            ),
        ] {
            let odata = act(
                &state,
                who,
                &format!("{entity}{action}"),
                Some(&etag),
                body.clone(),
            )
            .await;
            let rest_body = body.get("patch").cloned().unwrap_or(Value::Null);
            let request = Request::builder()
                .method(method)
                .uri("/api/v1/projects/helsinki/spaces/air?dryRun=All")
                .header(header::HOST, HEL)
                .header(header::COOKIE, &cookie)
                .header(csrf::CSRF_HEADER, &token)
                .header(header::CONTENT_TYPE, "application/merge-patch+json")
                .header(header::IF_MATCH, &etag)
                .body(if method == "PATCH" {
                    Body::from(rest_body.to_string())
                } else {
                    Body::empty()
                })
                .expect("request");
            let rest = server::app(state.clone())
                .oneshot(request)
                .await
                .expect("response")
                .status();
            if odata.status != rest {
                differ.push(format!(
                    "{who} {action}: OData {} {} / REST {method} {rest}",
                    odata.status, odata.text
                ));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

/// Critic §8 (T-6817): an entity answers `304` to its own `ETag` only after the read check (a
/// caller who may not read it keeps the 404), and every OData answer varies on who asks.
#[tokio::test]
async fn not_modified_comes_after_the_read_check_and_answers_vary_on_the_caller() {
    let state = state();
    let mode = Mode::Host("hel");
    let uri = "/api/v1/spaces(project='helsinki',name='air')";
    let one = get(&state, mode, "hel-only", uri).await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    let etag = one.headers[header::ETAG].to_str().expect("ETag").to_owned();
    let vary: Vec<&str> = one
        .headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        vary.iter().any(|v| v.contains("Authorization, Cookie")),
        "{vary:?}"
    );
    // `no-store` (the session layer's) is stricter than API/13's `private`: either keeps it out
    // of a shared cache.
    let cache = one.headers[header::CACHE_CONTROL]
        .to_str()
        .unwrap_or_default();
    assert!(matches!(cache, "private" | "no-store"), "{cache}");

    let reader = cookies(&state.config, person("hel-only", &[]), "hel").await;
    let same = send(&state, mode, &reader, uri, &[("if-none-match", &etag)]).await;
    assert_eq!(same.status, StatusCode::NOT_MODIFIED, "{}", same.text);
    assert!(same.text.is_empty());
    assert_eq!(same.headers[header::ETAG], etag.as_str());
    let other = send(
        &state,
        mode,
        &reader,
        uri,
        &[("if-none-match", "\"other\"")],
    )
    .await;
    assert_eq!(other.status, StatusCode::OK, "{}", other.text);

    // Another Organization's entity with the right tag: still not there.
    let elsewhere = state
        .mirror
        .get("bratislava", "ContextSpace", "air")
        .expect("air")
        .etag();
    let refused = send(
        &state,
        mode,
        &reader,
        "/api/v1/spaces(project='bratislava',name='air')",
        &[("if-none-match", &elsewhere)],
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    let nobody = send(&state, mode, "", uri, &[("if-none-match", &etag)]).await;
    assert_eq!(nobody.status, StatusCode::UNAUTHORIZED, "{}", nobody.text);
}

/// The registry's actions: a Change-proposing one needs the project's `If-Match` (428, then 412
/// with the current one) and then answers what the REST door answers; a dry run needs none; a
/// project opens only in the Organization whose service was called.
#[tokio::test]
async fn registry_actions_keep_their_door_and_need_the_projects_if_match() {
    let state = state();
    let project = get(
        &state,
        Mode::Host("hel"),
        "hel-editor",
        "/api/v1/projects('helsinki')",
    )
    .await;
    assert_eq!(project.status, StatusCode::OK, "{}", project.text);
    let etag = project
        .headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("a project answers its ETag")
        .to_owned();
    assert_eq!(
        project.json()["@odata.etag"],
        json!(etag),
        "{}",
        project.text
    );

    let delete = "/api/v1/projects('helsinki')/JoinedContext.Portal.jc_project_delete";
    let input = json!({ "name": "helsinki" });
    let missing = act(&state, "hel-editor", delete, None, input.clone()).await;
    assert_eq!(
        missing.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{}",
        missing.text
    );
    let stale = act(
        &state,
        "hel-editor",
        delete,
        Some("\"stale\""),
        input.clone(),
    )
    .await;
    assert_eq!(
        stale.status,
        StatusCode::PRECONDITION_FAILED,
        "{}",
        stale.text
    );
    assert_eq!(
        stale
            .headers
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some(etag.as_str())
    );
    // The editor may not delete the project: the REST door's 403, now that If-Match holds.
    let refused = act(&state, "hel-editor", delete, Some(&etag), input.clone()).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    // Another Organization's project is not there, If-Match or not.
    let elsewhere = act(
        &state,
        "hel-editor",
        "/api/v1/projects('bratislava')/JoinedContext.Portal.jc_project_delete",
        Some(&etag),
        json!({ "name": "bratislava" }),
    )
    .await;
    assert_eq!(
        elsewhere.status,
        StatusCode::NOT_FOUND,
        "{}",
        elsewhere.text
    );
    let stranger = act(&state, "ba-only", delete, Some(&etag), input).await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN, "{}", stranger.text);

    let create = "/api/v1/projects('helsinki')/JoinedContext.Portal.jc_project_create";
    let crossing = act(
        &state,
        "hel-editor",
        create,
        None,
        json!({ "name": "elsewhere", "organization": "ba" }),
    )
    .await;
    assert_eq!(
        crossing.status,
        StatusCode::BAD_REQUEST,
        "{}",
        crossing.text
    );

    // A dry run is no Change: no If-Match, the registry's answer as the REST door's.
    let dry = act(
        &state,
        "hel-editor",
        "/api/v1/projects('helsinki')/JoinedContext.Portal.jc_manifest_dry_run",
        None,
        json!({ "nothing": true }),
    )
    .await;
    assert_eq!(dry.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", dry.text);
    let editor = get(&state, Mode::Host("hel"), "hel-editor", "/api/v1/$metadata").await;
    assert!(
        editor
            .text
            .contains(r#"<Action Name="jc_manifest_dry_run" IsBound="true">"#),
        "{}",
        editor.text
    );
    assert!(
        !editor.text.contains(r#"Name="jc_resource_propose""#),
        "{}",
        editor.text
    );
}

async fn act(
    state: &AppState,
    who: &str,
    uri: &str,
    if_match: Option<&str>,
    body: Value,
) -> Answer {
    let cookie = cookies(&state.config, person(who, groups_of(who)), "hel").await;
    let token = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix(&format!("{CSRF_COOKIE}=")))
        .unwrap_or_default()
        .to_owned();
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, HEL)
        .header(header::COOKIE, &cookie)
        .header(csrf::CSRF_HEADER, token)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(tag) = if_match {
        request = request.header(header::IF_MATCH, tag);
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    Answer {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// Every entity carries its `@odata.etag` (a manifest's is the REST door's `ETag`), `$select` or
/// not, and one entity answers it as `ETag` too.
#[tokio::test]
async fn every_entity_carries_its_etag() {
    let state = state();
    let mode = Mode::Host("hel");
    let manifest = state
        .mirror
        .get("helsinki", "ContextSpace", "air")
        .expect("air");
    let one = get(
        &state,
        mode,
        "hel-only",
        "/api/v1/spaces(project='helsinki',name='air')",
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    assert_eq!(
        one.json()["@odata.etag"],
        json!(manifest.etag()),
        "{}",
        one.text
    );
    assert_eq!(
        one.headers.get(header::ETAG).and_then(|v| v.to_str().ok()),
        Some(manifest.etag().as_str())
    );
    let list = get(
        &state,
        mode,
        "hel-only",
        "/api/v1/projects('helsinki')/spaces?$select=name&$orderby=name",
    )
    .await
    .json();
    assert_eq!(
        list["value"][0]["@odata.etag"],
        json!(manifest.etag()),
        "{list}"
    );
    assert!(list["value"][0].get("_key").is_none(), "{list}");
}

/// A write is a bound action through the REST door's own check: If-Match first required (428),
/// then current (412 with the current ETag), only on an entity the caller reads (404 else); a
/// viewer is refused as REST refuses them; `$metadata` offers a viewer no action.
#[tokio::test]
async fn bound_writes_need_if_match_and_keep_the_rest_doors_rights() {
    let state = state();
    let etag = state
        .mirror
        .get("helsinki", "ContextSpace", "air")
        .expect("air")
        .etag();
    let update =
        "/api/v1/spaces(project='helsinki',name='air')/JoinedContext.Portal.propose_update";
    let patch = json!({ "patch": { "spec": {} }, "dryRun": true });

    let viewer = get(&state, Mode::Host("hel"), "hel-only", "/api/v1/$metadata").await;
    assert!(
        !viewer.text.contains("propose_update"),
        "a viewer's $metadata offers a write"
    );
    let editor = get(&state, Mode::Host("hel"), "hel-editor", "/api/v1/$metadata").await;
    assert!(
        editor
            .text
            .contains(r#"<Action Name="propose_update" IsBound="true">"#),
        "an editor's $metadata lacks the action"
    );
    let json = get(
        &state,
        Mode::Host("hel"),
        "hel-editor",
        "/api/v1/$metadata?$format=json",
    )
    .await;
    assert!(
        json.json()["JoinedContext.Portal"]["propose_update"].is_array(),
        "{}",
        json.text
    );

    let missing = act(&state, "hel-editor", update, None, patch.clone()).await;
    assert_eq!(
        missing.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{}",
        missing.text
    );
    let stale = act(
        &state,
        "hel-editor",
        update,
        Some("\"0000\""),
        patch.clone(),
    )
    .await;
    assert_eq!(
        stale.status,
        StatusCode::PRECONDITION_FAILED,
        "{}",
        stale.text
    );
    assert_eq!(
        stale
            .headers
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some(etag.as_str())
    );
    // Another Organization's entity is not there, whatever the precondition says.
    for tag in [None, Some(etag.as_str())] {
        let other = act(
            &state,
            "hel-editor",
            "/api/v1/spaces(project='bratislava',name='air')/JoinedContext.Portal.propose_update",
            tag,
            patch.clone(),
        )
        .await;
        assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    }
    let viewer = act(&state, "hel-only", update, Some(&etag), patch.clone()).await;
    assert_eq!(viewer.status, StatusCode::FORBIDDEN, "{}", viewer.text);
    assert_eq!(
        viewer.json()["error"]["status"],
        json!(403),
        "{}",
        viewer.text
    );
    let current = act(&state, "hel-editor", update, Some(&etag), patch.clone()).await;
    assert!(
        !matches!(
            current.status,
            StatusCode::PRECONDITION_REQUIRED
                | StatusCode::PRECONDITION_FAILED
                | StatusCode::NOT_FOUND
                | StatusCode::FORBIDDEN
        ),
        "the current ETag passes to the REST door: {} {}",
        current.status,
        current.text
    );
    let unknown = act(
        &state,
        "hel-editor",
        update,
        Some(&etag),
        json!({ "spec": {} }),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{}", unknown.text);
    let create = act(
        &state,
        "hel-editor",
        "/api/v1/projects('bratislava')/JoinedContext.Portal.propose_create",
        None,
        json!({ "manifest": { "kind": "ContextSpace" } }),
    )
    .await;
    assert_eq!(create.status, StatusCode::NOT_FOUND, "{}", create.text);
}

/// Writes other than actions answer 405 naming the action; a cookie write without the CSRF
/// token is refused before anything runs.
#[tokio::test]
async fn plain_writes_are_405_and_actions_need_the_csrf_token() {
    let state = state();
    let cookie = cookies(&state.config, person("hel-editor", &[]), "hel").await;
    for (method, uri) in [
        ("PATCH", "/api/v1/spaces(project='helsinki',name='air')"),
        ("PUT", "/api/v1/projects('helsinki')/spaces('air')"),
        ("POST", "/api/v1/spaces?$top=1"),
    ] {
        let response = server::app(state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::HOST, HEL)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri}"
        );
    }
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/spaces(project='helsinki',name='air')/JoinedContext.Portal.propose_delete")
                .header(header::HOST, HEL)
                .header(header::COOKIE, &cookie)
                .header(header::IF_MATCH, "*")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Red first (T-6822): an Organization's own Roles, RoleBindings and groups were no set of the
/// Organization service. Now each is a set keyed by name holding only that Organization's: a
/// RoleBinding is the Organization's its scope reaches, the rest of the `org` namespace the
/// installation's (hel). The other Organization's rows and keys answer nothing.
/// The `Member` entity type of the CSDL XML in `text`.
fn member_type(text: &str) -> &str {
    let start = text
        .find("<EntityType Name=\"Member\"")
        .unwrap_or(text.len());
    let end = text[start..]
        .find("</EntityType>")
        .map_or(text.len(), |n| start + n);
    &text[start..end]
}

/// ADR-N-070 section 2.4, PF-52, PF-58 (T-6826): an Organization's own RoleBindings and Roles
/// take `propose_update` and `propose_delete`, run through the REST door of `org`, so its rights,
/// its red lane and its four-eyes rule are that door's. `$metadata` offers them only to a caller
/// who may propose on the kind at the Organization; another Organization's row is 404; an
/// administrator's own access change waits for a second person (PF-58); 401 without a session.
#[tokio::test]
async fn organization_rows_take_red_lane_actions_through_the_rest_door() {
    let shako = common::forge().await;
    let client = ShakoClient::new(
        shako.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = state().with_shako(Arc::new(client));
    // The forge holds the binding's file, as a real proposal reads it before it removes it.
    let file = serde_norway::to_string(&envelope(
        "RoleBinding",
        "hel-only-hel",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "hel-only@hel.fi" }], "role": "reader",
                "scope": { "organization": "hel" } }),
    ))
    .expect("yaml");
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "sha-binding",
            "Content": base64::engine::general_purpose::STANDARD.encode(file),
        })))
        .with_priority(1)
        .mount(&shako)
        .await;
    let m = &state.mirror;
    m.upsert(envelope(
        "Role",
        "access-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "RoleBinding", "Role", "Group"],
                            "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    m.upsert(envelope(
        "Role",
        "binding-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "RoleBinding"], "verbs": ["read"] }] }),
    ));
    for (name, user, role) in [
        ("hel-admins", "sub-adm", "access-admin"),
        ("hel-bindings", "sub-acc", "binding-reader"),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": user }], "role": role,
                    "scope": { "organization": "hel" } }),
        ));
    }
    let tag = |kind: &str, name: &str| m.get(ORG_NAMESPACE, kind, name).expect(name).etag();
    let binding = "/api/v1/rolebindings('hel-only-hel')/JoinedContext.Portal.";
    let dry = json!({ "dryRun": true });

    let admin = get(&state, Mode::Host("hel"), "adm", "/api/v1/$metadata").await;
    let offered = |text: &str, ty: &str| {
        text.contains(&format!(
            r#"<Action Name="propose_delete" IsBound="true"><Parameter Name="bindingParameter" Type="JoinedContext.Portal.{ty}""#
        ))
    };
    for ty in ["RoleBinding", "OrganizationRole", "Group"] {
        assert!(offered(&admin.text, ty), "{ty}: {}", admin.text);
    }
    let reader = get(&state, Mode::Host("hel"), "acc", "/api/v1/$metadata").await;
    assert!(!offered(&reader.text, "RoleBinding"), "{}", reader.text);

    // A dry run answers the plan, in the red lane, and writes nothing.
    let etag = tag("RoleBinding", "hel-only-hel");
    for (action, body) in [
        ("propose_delete", dry.clone()),
        (
            "propose_update",
            json!({ "patch": { "spec": { "role": "editor" } }, "dryRun": true }),
        ),
    ] {
        let plan = act(
            &state,
            "adm",
            &format!("{binding}{action}"),
            Some(&etag),
            body,
        )
        .await;
        assert_eq!(plan.status, StatusCode::OK, "{action}: {}", plan.text);
        assert_eq!(plan.json()["lane"], "red", "{action}: {}", plan.text);
    }
    let role = "/api/v1/organizationRoles('reader')/JoinedContext.Portal.propose_delete";
    let plan = act(
        &state,
        "adm",
        role,
        Some(&tag("Role", "reader")),
        dry.clone(),
    )
    .await;
    assert_eq!(plan.status, StatusCode::OK, "{}", plan.text);
    assert_eq!(plan.json()["lane"], "red", "{}", plan.text);

    let delete = format!("{binding}propose_delete");
    let missing = act(&state, "adm", &delete, None, dry.clone()).await;
    assert_eq!(
        missing.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{}",
        missing.text
    );
    // A reader of the row without propose: the REST door's 403.
    let reader = act(&state, "acc", &delete, Some(&etag), dry.clone()).await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    // Who reads no RoleBinding, and ba's row read from hel: not there.
    let viewer = act(&state, "hel-only", &delete, Some(&etag), dry.clone()).await;
    assert_eq!(viewer.status, StatusCode::NOT_FOUND, "{}", viewer.text);
    let foreign = act(
        &state,
        "adm",
        "/api/v1/rolebindings('ba-only-ba')/JoinedContext.Portal.propose_delete",
        Some(&tag("RoleBinding", "ba-only-ba")),
        dry.clone(),
    )
    .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{}", foreign.text);

    // PF-58: the administrator's own access change is proposed and waits for a second person.
    let proposed = act(&state, "adm", &delete, Some(&etag), json!({})).await;
    assert_eq!(proposed.status, StatusCode::ACCEPTED, "{}", proposed.text);
    let change = proposed.json();
    assert_eq!(change["status"]["lane"], "red", "{}", proposed.text);
    assert_eq!(
        change["status"]["phase"], "PendingApproval",
        "{}",
        proposed.text
    );
    assert!(
        m.get(ORG_NAMESPACE, "RoleBinding", "hel-only-hel")
            .is_some(),
        "nothing is removed before a second person approves"
    );

    // No session (the CSRF pair present, so the guard lets it by): 401, whatever the action.
    let csrf_token = organization_host::scope(host("hel"), async { csrf::Token::mint() }).await;
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&delete)
                .header(header::HOST, HEL)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::IF_MATCH, &etag)
                .header(
                    header::COOKIE,
                    format!("{CSRF_COOKIE}={}", csrf_token.as_str()),
                )
                .header(csrf::CSRF_HEADER, csrf_token.as_str())
                .body(Body::from(dry.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// [`get`] as an OData request, which a plain read without a `$` option is not.
async fn odata(state: &AppState, mode: Mode, who: &str, uri: &str) -> Answer {
    let cookie = cookies(&state.config, person(who, groups_of(who)), mode.org()).await;
    send(state, mode, &cookie, uri, &[("OData-Version", "4.0")]).await
}

/// ADR-N-070 section 2.8, ADR-N-061 (T-6826): `members` lists the people the Organization's
/// RoleBindings name, directly or through a Group. Only a people-admin has their name and e-mail,
/// as properties and as values; to anyone else a personal property is what a typo is, the same
/// 400. A realm change to a person's e-mail moves the people-admin's ETag of the row and leaves
/// everyone else's.
#[tokio::test]
async fn members_carry_personal_data_only_for_people_admins() {
    let kc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/hel/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&kc)
        .await;
    let email = Arc::new(Mutex::new("ada@hel.fi".to_owned()));
    let current = email.clone();
    Mock::given(method("GET"))
        .and(path("/admin/realms/hel/users/sub-ada"))
        .respond_with(move |_: &wiremock::Request| {
            let email = current.lock().map(|e| e.clone()).unwrap_or_default();
            ResponseTemplate::new(200).set_body_json(json!({ "id": "sub-ada", "username": "ada",
                "firstName": "Ada", "lastName": "Lovelace", "email": email }))
        })
        .mount(&kc)
        .await;
    let mut state = state();
    state.people = People::new(
        &format!("{}/realms/hel", kc.uri()),
        "portal".into(),
        ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);
    let m = &state.mirror;
    for (role, kind) in [
        ("binding-reader", "RoleBinding"),
        ("person-reader", "Person"),
    ] {
        m.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Project", kind], "verbs": ["read"] }] }),
        ));
    }
    for (name, subject, role) in [
        (
            "hel-bindings",
            json!({ "user": "sub-acc" }),
            "binding-reader",
        ),
        ("hel-people", json!({ "user": "sub-ppl" }), "person-reader"),
        ("hel-team", json!({ "group": "team" }), "reader"),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [subject], "role": role, "scope": { "organization": "hel" } }),
        ));
    }
    m.upsert(envelope(
        "Group",
        "team",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "sub-ada" }] }),
    ));
    let without_trace = |answer: &Answer| {
        let mut body = answer.json();
        if let Some(error) = body["error"].as_object_mut() {
            error.remove("trace_id");
        }
        body.to_string()
    };
    for mode in [Mode::Host("hel"), Mode::Path("hel")] {
        // A people-admin: name and e-mail are properties and values; an old e-mail subject too.
        let rows = odata(&state, mode, "ppl", "/api/v1/members").await;
        assert_eq!(rows.status, StatusCode::OK, "{mode:?}: {}", rows.text);
        let body = rows.json();
        let ada = body["value"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["user"] == "sub-ada"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(ada["email"], "ada@hel.fi", "{mode:?}: {}", rows.text);
        assert_eq!(ada["name"], "Ada Lovelace", "{mode:?}");
        assert_eq!(ada["roles"], "reader", "{mode:?}");
        assert_eq!(ada["bindings"], "hel-team", "{mode:?}");
        let users = names(&body, "user");
        assert!(users.contains(&"hel-only@hel.fi".to_owned()), "{users:?}");
        assert!(
            !users.iter().any(|u| u.starts_with("ba-only")),
            "{mode:?}: ba's people in hel's members: {users:?}"
        );
        let found = odata(
            &state,
            mode,
            "ppl",
            "/api/v1/members?$filter=email%20eq%20'ada@hel.fi'&$select=user",
        )
        .await;
        assert_eq!(
            names(&found.json(), "user"),
            vec!["sub-ada"],
            "{}",
            found.text
        );
        let metadata = odata(&state, mode, "ppl", "/api/v1/$metadata").await;
        assert!(
            member_type(&metadata.text).contains("Name=\"email\""),
            "{}",
            metadata.text
        );

        // A RoleBinding reader: the people, never their name or e-mail, nor an e-mail subject.
        let rows = odata(&state, mode, "acc", "/api/v1/members").await;
        assert_eq!(rows.status, StatusCode::OK, "{mode:?}: {}", rows.text);
        let body = rows.json();
        let users = names(&body, "user");
        assert!(users.contains(&"sub-ada".to_owned()), "{users:?}");
        assert!(users.iter().all(|u| !u.contains('@')), "{users:?}");
        for row in body["value"].as_array().into_iter().flatten() {
            assert!(
                row.get("email").is_none() && row.get("name").is_none(),
                "{mode:?}: {row}"
            );
        }
        let metadata = odata(&state, mode, "acc", "/api/v1/$metadata").await;
        let member = member_type(&metadata.text);
        assert!(member.contains("Name=\"user\""), "{}", metadata.text);
        assert!(
            !member.contains("Name=\"email\"") && !member.contains("Name=\"name\""),
            "{member}"
        );
        for option in [
            "$filter=email%20eq%20'x'",
            "$select=email",
            "$orderby=email",
        ] {
            let hidden = odata(&state, mode, "acc", &format!("/api/v1/members?{option}")).await;
            let typo = option.replace("email", "emailx");
            let typo = odata(&state, mode, "acc", &format!("/api/v1/members?{typo}")).await;
            assert_eq!(
                hidden.status,
                StatusCode::BAD_REQUEST,
                "{option}: {}",
                hidden.text
            );
            assert_eq!(
                without_trace(&hidden),
                without_trace(&typo).replace("emailx", "email"),
                "{option}"
            );
        }

        // The e-mail changes in the realm: only the people-admin's ETag moves.
        let etag = |answer: Answer| answer.json()["@odata.etag"].as_str().map(str::to_owned);
        let row = "/api/v1/members('sub-ada')";
        let before_acc = etag(odata(&state, mode, "acc", row).await);
        let before_ppl = etag(odata(&state, mode, "ppl", row).await);
        assert!(before_acc.is_some() && before_ppl.is_some());
        let old = email.lock().map(|e| e.clone()).unwrap_or_default();
        if let Ok(mut e) = email.lock() {
            *e = format!("{old}.new");
        }
        assert_eq!(
            etag(odata(&state, mode, "acc", row).await),
            before_acc,
            "{mode:?}"
        );
        assert_ne!(
            etag(odata(&state, mode, "ppl", row).await),
            before_ppl,
            "{mode:?}"
        );
        if let Ok(mut e) = email.lock() {
            *e = old;
        }

        // A reader of neither RoleBinding nor Person has no members set.
        let viewer = odata(&state, mode, "hel-only", "/api/v1/members").await;
        assert_eq!(viewer.status, StatusCode::NOT_FOUND, "{}", viewer.text);
        let metadata = odata(&state, mode, "hel-only", "/api/v1/$metadata").await;
        assert!(
            !metadata.text.contains("EntitySet Name=\"members\""),
            "{mode:?}"
        );
    }
    // Another Organization's host: hel's RoleBinding reader is turned away at its door, and
    // ba's own administrator reads ba's people only.
    let ba = odata(&state, Mode::Host("ba"), "acc", "/api/v1/members").await;
    assert_eq!(ba.status, StatusCode::FORBIDDEN, "{}", ba.text);
    let ba = odata(&state, Mode::Host("ba"), "root", "/api/v1/members").await;
    let users = names(&ba.json(), "user");
    assert!(users.contains(&"ba-only@hel.fi".to_owned()), "{}", ba.text);
    assert!(
        !users
            .iter()
            .any(|u| u.starts_with("hel-") || u.starts_with("sub-")),
        "{users:?}"
    );
}

#[tokio::test]
async fn organization_sets_hold_only_the_organizations_own_rows() {
    let state = state();
    let m = &state.mirror;
    m.upsert(envelope(
        "Role",
        "access-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "RoleBinding", "Role", "Group"], "verbs": ["read"] }] }),
    ));
    for org in ["hel", "ba"] {
        m.upsert(envelope(
            "RoleBinding",
            &format!("{org}-access-{org}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{org}-access@hel.fi") }],
                    "role": "access-reader", "scope": { "organization": org } }),
        ));
    }
    m.upsert(envelope(
        "RoleBinding",
        "air-reader-ba",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "ba-only@hel.fi" }], "role": "reader",
                "scope": { "project": "bratislava" } }),
    ));
    m.upsert(envelope(
        "Group",
        "noise-team",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "hel-only@hel.fi" }] }),
    ));
    for mode in MODES {
        let org = mode.org();
        let (own, other) = if org == "hel" {
            ("hel", "ba")
        } else {
            ("ba", "hel")
        };
        for who in ["root", &format!("{own}-access")] {
            let bindings = get(&state, mode, who, "/api/v1/rolebindings?$select=name").await;
            assert_eq!(
                bindings.status,
                StatusCode::OK,
                "{who} {mode:?}: {}",
                bindings.text
            );
            let names = names(&bindings.json(), "name");
            assert!(
                names.contains(&format!("{own}-access-{own}")),
                "{who} {mode:?}: {names:?}"
            );
            assert!(
                names.iter().all(|n| !n.ends_with(&format!("-{other}"))),
                "{who} {mode:?} reads {other}'s bindings: {names:?}"
            );
            assert_eq!(
                names.contains(&"air-reader-ba".to_owned()),
                own == "ba",
                "a project binding is its project's Organization's: {who} {mode:?}"
            );
            let foreign = format!("/api/v1/rolebindings('{other}-access-{other}')");
            let foreign = get(&state, mode, who, &foreign).await;
            assert_eq!(
                foreign.status,
                StatusCode::NOT_FOUND,
                "{who} {mode:?}: {}",
                foreign.text
            );
            let mine = format!("/api/v1/rolebindings('{own}-access-{own}')");
            let mine = get(&state, mode, who, &mine).await;
            assert_eq!(mine.status, StatusCode::OK, "{who} {mode:?}: {}", mine.text);
            assert!(mine.json()["@odata.etag"].is_string(), "{}", mine.text);
            let count = get(&state, mode, who, "/api/v1/rolebindings/$count").await;
            assert_eq!(count.text, names.len().to_string(), "{who} {mode:?}");

            let metadata = get(&state, mode, who, "/api/v1/$metadata").await;
            for (set, path) in [
                (
                    "organizationRoles",
                    "/api/v1/organizationRoles?$select=name",
                ),
                ("groups", "/api/v1/groups?$select=name"),
            ] {
                let answer = get(&state, mode, who, path).await;
                let listed = metadata.text.contains(&format!("EntitySet Name=\"{set}\""));
                if org == "hel" {
                    assert_eq!(
                        answer.status,
                        StatusCode::OK,
                        "{who} {mode:?} {set}: {}",
                        answer.text
                    );
                    assert!(listed, "{who} {mode:?}: {set} not in $metadata");
                } else {
                    assert_eq!(
                        answer.status,
                        StatusCode::NOT_FOUND,
                        "{who} {mode:?} {set}: {}",
                        answer.text
                    );
                    assert!(!listed, "{who} {mode:?}: {set} in ba's $metadata");
                }
            }
            assert!(
                metadata.text.contains("EntitySet Name=\"rolebindings\""),
                "{who} {mode:?}"
            );
        }
        let roles = get(
            &state,
            Mode::Host("hel"),
            "root",
            "/api/v1/organizationRoles?$select=name",
        )
        .await;
        let roles = names(&roles.json(), "name");
        assert!(
            roles.contains(&"reader".to_owned()) && roles.contains(&"access-reader".to_owned()),
            "{roles:?}"
        );
        // A viewer whose Role reads no RoleBinding has no such set and its URL answers 404.
        let viewer = get(
            &state,
            mode,
            &format!("{own}-only"),
            "/api/v1/rolebindings?$select=name",
        )
        .await;
        assert_eq!(
            viewer.status,
            StatusCode::NOT_FOUND,
            "{mode:?}: {}",
            viewer.text
        );
        let metadata = get(&state, mode, &format!("{own}-only"), "/api/v1/$metadata").await;
        assert!(!metadata.text.contains("\"rolebindings\""), "{mode:?}");
    }
}
