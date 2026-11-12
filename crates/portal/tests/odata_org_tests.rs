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
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use common::envelope;
use joinedcontext_portal::auth::csrf::{self, CSRF_COOKIE};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::organization_host::{self, OrganizationHost, RequestOrganization};
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
        Some(vec!["name".to_owned()]),
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
