//! A request on an Organization's own host (`HostOrganization`, which `jc` places from the host
//! its router resolved): every listing holds that Organization's public Endpoints alone, and an
//! Endpoint, space or MCP server of another Organization is the 404 an unknown one is. The
//! hub's remembered reach is kept per host Organization, so one host's list never answers
//! another's. Without the extension (one Organization per gateway) nothing changes.

use crate::common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use context_gateway::app::{router, Gateway, HostBase, HostOrganization};
use context_gateway::auth::accounts::ServiceAccounts;
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::{Endpoint, Space};
use context_gateway::store::Provenance;
use jc_core::kinds::{Audience, PolicySpec, Representation};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// alpha's public Endpoint, in project `pa`.
const ALPHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
/// beta's public Endpoint, in project `pb`.
const BETA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbb";

fn endpoint(slug: &str, title: &str, project: &str) -> Endpoint {
    let space = format!("space-{project}");
    let policy: PolicySpec = serde_norway::from_str(&format!(
        "contextSpaceRef: {space}\n\
         assigner: did:web:city.test\n\
         assignee: {{ kind: role, id: public }}\n\
         operations: [queryEntity, retrieveEntity]\n\
         information:\n  - entities:\n      - type: AirQualityObserved\n"
    ))
    .expect("the policy spec parses");
    Endpoint {
        declared_types: None,
        roles: Default::default(),
        slug: slug.to_owned(),
        title: [("en".to_owned(), title.to_owned())].into(),
        description: [("en".to_owned(), format!("{title} air"))].into(),
        space,
        project: project.to_owned(),
        audience: Audience::Public,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd, Representation::Mcp],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: Default::default(),
        projection: None,
        base_path: format!("/api/endpoint/{slug}"),
        models: Vec::new(),
        view_mapping: None,
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![policy],
    }
}

/// The public space `space-{project}`, which `/cs` lists anonymously.
fn space(project: &str) -> Space {
    let name = format!("space-{project}");
    let mut endpoint = endpoint(&name, &format!("Space of {project}"), project);
    endpoint.base_path = format!("/cs/{name}");
    Space {
        endpoint: Arc::new(endpoint),
        title: [("en".to_owned(), format!("Space of {project}"))].into(),
        description: Default::default(),
        is_sandbox: false,
        default_locale: None,
    }
}

fn app(realm: &common::Realm) -> Router {
    let gateway = Gateway::new(
        Broker::new("http://127.0.0.1:1"),
        Box::new(PolicyPdp),
        "city.test",
    )
    .serve([
        endpoint(ALPHA, "Alphabox", "pa"),
        endpoint(BETA, "Betabox", "pb"),
    ])
    .serve_spaces([space("pa"), space("pb")])
    .authenticate(
        Arc::new(realm.verifier()),
        ServiceAccounts::new(),
        Some("https://api.city.test".to_owned()),
    );
    gateway.replace_provenance(Provenance {
        organizations: [("pa", "alpha"), ("pb", "beta")]
            .into_iter()
            .map(|(p, o)| (p.to_owned(), o.to_owned()))
            .collect(),
        ..Default::default()
    });
    router(Arc::new(gateway))
}

/// One request as `jc` hands it over: on `host`'s Organization, or on none.
async fn send(
    app: &Router,
    host: Option<&str>,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("a request");
    if let Some(host) = host {
        request
            .extensions_mut()
            .insert(HostOrganization(host.to_owned()));
    }
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("a body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn tool(name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    })
}

/// The slugs the anonymous hub's `list_endpoints` answers on `host`.
async fn listed(app: &Router, host: Option<&str>) -> Vec<String> {
    let (_, text) = send(
        app,
        host,
        Method::POST,
        "/api/mcp",
        Some(tool("list_endpoints", json!({}))),
    )
    .await;
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    answer["result"]["structuredContent"]["endpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("no list on {host:?}: {text}"))
        .iter()
        .map(|e| e["slug"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The anonymous hub list per host: alpha first, then beta inside the reach cache's lifetime, so
/// a cache keyed without the host's Organization answers beta with alpha's list.
#[tokio::test]
async fn the_anonymous_hub_lists_only_the_hosts_organization() {
    let realm = common::Realm::new();
    let app = app(&realm);
    assert_eq!(listed(&app, Some("alpha")).await, [ALPHA]);
    assert_eq!(listed(&app, Some("beta")).await, [BETA]);
    assert_eq!(listed(&app, Some("alpha")).await, [ALPHA]);
    // No Organization's host: the installation's whole list, as before.
    assert_eq!(listed(&app, None).await, [ALPHA, BETA]);
    // A host of an Organization with no Endpoint lists none.
    assert!(listed(&app, Some("gamma")).await.is_empty());
}

#[tokio::test]
async fn every_listing_on_one_host_holds_nothing_of_the_other_organization() {
    let realm = common::Realm::new();
    let app = app(&realm);
    for (host, own, other, other_title) in [
        ("alpha", ALPHA, BETA, "Betabox"),
        ("beta", BETA, ALPHA, "Alphabox"),
    ] {
        let space = if other == BETA {
            "space-pb"
        } else {
            "space-pa"
        };
        for path in ["/api/endpoints", "/catalog.jsonld", "/catalog.ttl", "/cs"] {
            let (status, body) = send(&app, Some(host), Method::GET, path, None).await;
            assert_eq!(status, StatusCode::OK, "{host}{path}: {body}");
            assert!(
                !body.contains(other) && !body.contains(other_title) && !body.contains(space),
                "{host}{path} shows the other Organization: {body}"
            );
        }
        let (_, body) = send(&app, Some(host), Method::GET, "/catalog.jsonld", None).await;
        assert!(body.contains(own), "{host}: {body}");
        let own_space = if own == ALPHA { "space-pa" } else { "space-pb" };
        let (_, body) = send(&app, Some(host), Method::GET, "/cs", None).await;
        assert!(body.contains(own_space), "{host}/cs: {body}");
        // The hub's tools reach no Endpoint of the other Organization either.
        for (name, arguments) in [
            ("get_endpoint", json!({ "endpoint": other })),
            ("search", json!({ "query": format!("{other_title} air") })),
        ] {
            let (_, text) = send(
                &app,
                Some(host),
                Method::POST,
                "/api/mcp",
                Some(tool(name, arguments)),
            )
            .await;
            assert!(!text.contains(other_title), "{host} {name}: {text}");
        }
        // Its addresses are the 404 of an unknown Endpoint, space or server.
        let (unknown, unknown_body) = send(
            &app,
            Some(host),
            Method::GET,
            "/api/endpoint/zzzzzzzzzzzzzzzzzzzzzzzzzz/",
            None,
        )
        .await;
        let project = if other == BETA { "pb" } else { "pa" };
        for path in [
            format!("/api/endpoint/{other}/"),
            format!("/api/endpoint/{other}/ngsi-ld/v1/entities?type=AirQualityObserved"),
            format!("/api/endpoint/{other}/access"),
            format!("/cs/{space}"),
            format!("/api/mcp/{project}/x"),
        ] {
            let (status, body) = send(&app, Some(host), Method::GET, &path, None).await;
            assert_eq!(
                (status, body.as_str()),
                (unknown, unknown_body.as_str()),
                "{host}{path}"
            );
        }
        // Its own record answers on its own host.
        let (status, body) = send(
            &app,
            Some(host),
            Method::GET,
            &format!("/api/endpoint/{own}/"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
    }
    // Without a host Organization both records answer.
    for slug in [ALPHA, BETA] {
        let (status, body) = send(
            &app,
            None,
            Method::GET,
            &format!("/api/endpoint/{slug}/"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{slug}: {body}");
    }
}

/// jc-multiorg-portal: with the host's link base beside its Organization (`HostBase`), every
/// answer on beta's API host links beta's host and never the one configured public URL; without
/// it the configured URL stays, as before.
#[tokio::test]
async fn an_answer_on_one_organizations_host_links_only_that_host() {
    let realm = common::Realm::new();
    let app = app(&realm);
    let on = |base: Option<&'static str>, path: &'static str| {
        let app = app.clone();
        async move {
            let mut request = Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .expect("a request");
            request
                .extensions_mut()
                .insert(HostOrganization("beta".to_owned()));
            if let Some(base) = base {
                request.extensions_mut().insert(HostBase(base.to_owned()));
            }
            let response = app.oneshot(request).await.expect("an answer");
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .expect("a body");
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };
    for path in ["/api/endpoints", "/catalog.jsonld", "/cs"] {
        let (status, text) = on(Some("http://api.beta.test:8080"), path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {text}");
        assert!(
            text.contains("http://api.beta.test:8080/"),
            "{path} links beta's host: {text}"
        );
        assert!(
            !text.contains("api.city.test"),
            "{path} links the configured URL: {text}"
        );
        let (_, control) = on(None, path).await;
        assert!(
            control.contains("https://api.city.test/"),
            "{path} without a host base: {control}"
        );
    }
}
