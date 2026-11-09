//! jc-multiorg-portal: one Portal, two Organizations, each on a Portal host of its own
//! (`JC_PORTAL_ORGANIZATION_HOSTS`). Through the whole router, with the `Host` a browser sends:
//!
//! - a host lists and opens only its own Organization's projects, for everyone, the bootstrap
//!   group included (Decided: scoped to that host's Organization);
//! - a person of the other Organization signed in there (an SSO session) gets 403 on the API;
//! - a session cookie and a CSRF token of one host are refused on the other.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use common::envelope;
use joinedcontext_portal::auth::csrf::{self, CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::organization_host::{self, OrganizationHost};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const HEL: &str = "portal.hel.example";
const BA: &str = "portal.ba.example";
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

/// Two Organizations, hel (the installation) and ba, a project each; `hel-only` is bound in
/// hel, `ba-only` in ba, `both` in both, `root` is in the bootstrap group.
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
        json!({ "rules": [{ "kinds": ["*"], "verbs": ["read", "propose"] }] }),
    ));
    m.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    m.upsert(envelope("Organization", "ba", ORG_NAMESPACE, json!({})));
    for (project, owner) in [("helsinki", "hel"), ("bratislava", "ba")] {
        m.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({ "organizationRef": owner }),
        ));
        m.upsert(envelope(
            "ContextSpace",
            &format!("{project}-air"),
            project,
            json!({}),
        ));
    }
    for (who, org) in [
        ("hel-only", "hel"),
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

/// The cookies a browser holds after signing in on `on` (`None`: the deployment's own address):
/// the session and the CSRF token, both minted on that host.
async fn signed_in(config: &Config, identity: Identity, on: Option<&str>) -> (String, String) {
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
        (parts.join("; "), token.as_str().to_owned())
    };
    match on {
        Some(name) => organization_host::scope(host(name), mint).await,
        None => mint.await,
    }
}

async fn call(
    state: &AppState,
    at: &str,
    cookies: &(String, String),
    method: &str,
    uri: &str,
) -> (StatusCode, Value) {
    let body = if method == "GET" {
        Body::empty()
    } else {
        Body::from(json!({}).to_string())
    };
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, at)
                .header(header::COOKIE, &cookies.0)
                .header(CSRF_HEADER, &cookies.1)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn names(list: &Value) -> Vec<&str> {
    list["items"]
        .as_array()
        .map(|items| items.iter().filter_map(|i| i["name"].as_str()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn each_host_lists_and_opens_only_its_own_organizations_projects() {
    let state = state();
    for (who, groups) in [("both", &[][..]), ("root", &[ROOTS][..])] {
        let on_ba = signed_in(&state.config, person(who, groups), Some("ba")).await;
        let (st, list) = call(&state, BA, &on_ba, "GET", "/api/v1/projects").await;
        assert_eq!(st, StatusCode::OK, "{who}: {list}");
        assert!(names(&list).contains(&"bratislava"), "{who}: {list}");
        assert!(
            !names(&list).contains(&"helsinki"),
            "{who} sees hel on ba's host: {list}"
        );
        assert!(
            list["organizations"].as_array().is_none_or(Vec::is_empty),
            "{who}: one Organization on its host needs no switcher: {list}"
        );
        for uri in [
            "/api/v1/projects/helsinki",
            "/api/v1/projects/helsinki/spaces",
            "/api/v1/projects/helsinki/spaces/helsinki-air",
        ] {
            let (st, body) = call(&state, BA, &on_ba, "GET", uri).await;
            assert!(
                matches!(st, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
                "{who} {uri} on ba's host: {st} {body}"
            );
        }
        let (st, body) = call(
            &state,
            BA,
            &on_ba,
            "GET",
            "/api/v1/projects/bratislava/spaces",
        )
        .await;
        assert_eq!(
            st,
            StatusCode::OK,
            "{who}: the control, own project: {body}"
        );
        // And the other way round.
        let on_hel = signed_in(&state.config, person(who, groups), Some("hel")).await;
        let (_, list) = call(&state, HEL, &on_hel, "GET", "/api/v1/projects").await;
        assert!(
            names(&list).contains(&"helsinki") && !names(&list).contains(&"bratislava"),
            "{who}: {list}"
        );
        let (st, _) = call(&state, HEL, &on_hel, "GET", "/api/v1/projects/bratislava").await;
        assert!(
            matches!(st, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{who}: {st}"
        );
    }
}

/// SSO: a person of hel alone, signed in on ba's Portal host through the shared realm session,
/// holds a ba session and is refused every API call there.
#[tokio::test]
async fn a_person_of_another_organization_signed_in_on_its_host_gets_403() {
    let state = state();
    let on_ba = signed_in(&state.config, person("hel-only", &[]), Some("ba")).await;
    for uri in [
        "/api/v1/projects",
        "/api/v1/projects/bratislava",
        "/api/v1/projects/helsinki",
    ] {
        let (st, body) = call(&state, BA, &on_ba, "GET", uri).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}: {body}");
    }
    // Signing out stays open to them.
    let (st, _) = call(&state, BA, &on_ba, "POST", "/api/v1/auth/logout").await;
    assert_ne!(st, StatusCode::FORBIDDEN);
    // On their own host they read their own.
    let on_hel = signed_in(&state.config, person("hel-only", &[]), Some("hel")).await;
    let (st, list) = call(&state, HEL, &on_hel, "GET", "/api/v1/projects").await;
    assert_eq!((st, names(&list)), (StatusCode::OK, vec!["helsinki"]));
}

/// One cookie key seals every host's cookies: the session of hel's host, presented on ba's (or on
/// the deployment's own address), is no session there.
#[tokio::test]
async fn a_session_cookie_of_one_host_is_no_session_on_another() {
    let state = state();
    let on_hel = signed_in(&state.config, person("both", &[]), Some("hel")).await;
    let (st, _) = call(&state, HEL, &on_hel, "GET", "/api/v1/projects").await;
    assert_eq!(st, StatusCode::OK, "the control: on its own host");
    let (st, _) = call(&state, BA, &on_hel, "GET", "/api/v1/projects").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "hel's cookie on ba's host");
    let (st, _) = call(&state, "portal.example", &on_hel, "GET", "/api/v1/projects").await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "hel's cookie on the deployment's own address"
    );
    let plain = signed_in(&state.config, person("both", &[]), None).await;
    let (st, _) = call(&state, BA, &plain, "GET", "/api/v1/projects").await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "an unpinned cookie on ba's host"
    );
}

/// The CSRF token is the host's too: ba's session with hel's double-submitted token is refused
/// a write; with its own it reaches the route.
#[tokio::test]
async fn a_csrf_token_of_one_host_is_refused_on_another() {
    let state = state();
    let on_ba = signed_in(&state.config, person("both", &[]), Some("ba")).await;
    let on_hel = signed_in(&state.config, person("both", &[]), Some("hel")).await;
    // ba's session, hel's token in both the cookie and the header.
    let session_only = on_ba
        .0
        .split("; ")
        .filter(|c| !c.starts_with(CSRF_COOKIE))
        .collect::<Vec<_>>()
        .join("; ");
    let mixed = (
        format!("{session_only}; {CSRF_COOKIE}={}", on_hel.1),
        on_hel.1.clone(),
    );
    let uri = "/api/v1/projects/bratislava/workspaces";
    let (st, body) = call(&state, BA, &mixed, "POST", uri).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "hel's token on ba's host: {body}"
    );
    let (st, body) = call(&state, BA, &on_ba, "POST", uri).await;
    assert_ne!(
        st,
        StatusCode::FORBIDDEN,
        "ba's own token passes the CSRF check: {body}"
    );
}
