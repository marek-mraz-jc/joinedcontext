#![forbid(unsafe_code)]
//! What a caller with no identity, or no Organization, gets (WRK-04, TEN-05): a public dashboard
//! reads the host's gateway Endpoints without a credential, so it shows what their public Policy
//! allows and nothing else, and a request that names no Organization is refused, never served as
//! the main one.

mod common;

use axum::body::Body;
use base64::Engine as _;
use common::{ORG, OTHER_ORG, World, dashboard, prometheus, upstream, world};
use daiya_api::{DaiyaState, api_router};
use daiya_model::{DaiyaStore as _, ShareType, Subject};
use daiya_query::{EndpointBase, OwnReach};
use http::{Request, StatusCode};
use jc_router::Mount;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

const ADMIN_PASSWORD: &str = "0123456789abcdef-admin";

fn mounted(mut w: World, change: impl FnOnce(DaiyaState) -> DaiyaState) -> World {
    w.state = change(w.state);
    w.app = api_router(
        w.state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    w
}

/// A world whose gateway serves the Endpoint `open` to anyone and knows no other for them.
async fn with_gateway() -> (World, String, Arc<Mutex<Vec<String>>>) {
    let (url, seen) = upstream(vec![(
        "/api/endpoint/open/ngsi-ld/v1/entities",
        json!([{ "id": "urn:ngsi-ld:Park:shown", "type": "Park",
                 "area": { "type": "Property", "value": 7 } }]),
    )])
    .await;
    let reach = OwnReach::endpoints(&format!("{url}/api/endpoint/")).unwrap();
    let w = mounted(world(), |mut state| {
        state.own_reach = vec![reach];
        state.endpoint_bases =
            vec![EndpointBase::parse("https://api.city.example/api/endpoint/").unwrap()];
        state
    });
    (w, url, seen)
}

/// Publishes dashboard `uid` with one panel reading `ds`, and answers its token.
async fn publish(w: &World, uid: &str, ds: &str, target: Value) -> String {
    let mut board = dashboard(uid, ds);
    board["panels"][0]["datasource"] = json!({ "uid": ds });
    board["panels"][0]["targets"] = json!([target]);
    w.add_dashboard(board, None).await;
    let made = w
        .ok(
            "POST",
            &format!("/api/dashboards/uid/{uid}/public-dashboards"),
            Some(json!({ "isEnabled": true })),
        )
        .await;
    made["accessToken"].as_str().unwrap().to_owned()
}

async fn read_panel(w: &World, token: &str) -> (StatusCode, Value) {
    w.call(
        "POST",
        &format!("/api/public/dashboards/{token}/panels/1/query"),
        None,
        Some(json!({})),
    )
    .await
}

fn entities(ds: &str) -> Value {
    json!({ "refId": "A", "queryType": "entity", "entityType": "Park",
            "datasource": { "uid": ds } })
}

async fn add_ngsild(w: &World, uid: &str, url: &str) {
    w.ok(
        "POST",
        "/api/datasources",
        Some(json!({ "uid": uid, "name": uid, "type": "ngsild-grafana-datasource",
                     "url": url, "jsonData": { "oauthPassThru": true } })),
    )
    .await;
}

#[tokio::test]
async fn a_public_panel_shows_what_the_endpoints_public_policy_allows_and_nothing_else() {
    let (w, url, seen) = with_gateway().await;
    add_ngsild(&w, "open", &format!("{url}/api/endpoint/open")).await;
    add_ngsild(&w, "closed", &format!("{url}/api/endpoint/closed")).await;
    let open = publish(&w, "open-board", "open", entities("open")).await;
    let closed = publish(&w, "closed-board", "closed", entities("closed")).await;

    let (st, body) = read_panel(&w, &open).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("urn:ngsi-ld:Park:shown"), "{body}");

    // The gateway has no public answer for the closed Endpoint, so the panel has none.
    let (st, body) = read_panel(&w, &closed).await;
    assert!(!st.is_success(), "{st} {body}");
    assert!(!body.to_string().contains("urn:ngsi-ld"), "{body}");

    let asked = seen.lock();
    assert_eq!(asked.len(), 2, "{asked:?}");
    for head in asked.iter() {
        let head = head.to_ascii_lowercase();
        assert!(!head.contains("authorization:"), "{head}");
    }
}

#[tokio::test]
async fn a_public_panel_reads_no_source_that_is_not_an_endpoint() {
    let (w, gateway, gateway_seen) = with_gateway().await;
    let (prom, prom_seen) = prometheus().await;
    w.add_prometheus("prom", &prom).await;
    // An NGSI-LD source at an address that is no Endpoint of this host's gateway.
    let (elsewhere, elsewhere_seen) = upstream(vec![("/", json!([]))]).await;
    add_ngsild(&w, "elsewhere", &format!("{elsewhere}/api/endpoint/open")).await;
    add_ngsild(&w, "beside", &format!("{gateway}/api/admin")).await;
    for (uid, target) in [
        ("prom", json!({ "refId": "A", "expr": "up", "datasource": { "uid": "prom" } })),
        ("elsewhere", entities("elsewhere")),
        ("beside", entities("beside")),
    ] {
        let token = publish(&w, &format!("{uid}-board"), uid, target).await;
        let (st, body) = read_panel(&w, &token).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uid}: {body}");
        let text = body.to_string();
        assert!(text.contains("public Endpoint"), "{uid}: {text}");
        assert!(!text.contains("127.0.0.1"), "{uid}: {text}");
    }
    for seen in [&prom_seen, &elsewhere_seen, &gateway_seen] {
        assert!(seen.lock().is_empty(), "{:?}", seen.lock());
    }
}

#[tokio::test]
async fn a_driver_the_host_registers_is_not_a_public_source() {
    let (w, url, seen) = with_gateway().await;
    let endpoint = format!("{url}/api/endpoint/open");
    add_ngsild(&w, "open", &endpoint).await;
    let record = w.store.get_datasource(ORG, "open").await.unwrap().unwrap();
    let driver = w.state.driver_for_record(&record).await.unwrap();
    w.state.register_datasource_driver(ORG, "open", driver);
    let token = publish(&w, "board", "open", entities("open")).await;
    let (st, body) = read_panel(&w, &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert!(seen.lock().is_empty());
}

#[tokio::test]
async fn a_share_by_email_is_refused_and_never_served_as_public() {
    let (w, url, _) = with_gateway().await;
    add_ngsild(&w, "open", &format!("{url}/api/endpoint/open")).await;
    let token = publish(&w, "board", "open", entities("open")).await;
    let (st, body) = w
        .call(
            "POST",
            "/api/dashboards/uid/board/public-dashboards",
            Some("admin"),
            Some(json!({ "share": "email" })),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.to_string().contains("share"), "{body}");
    let mut held = w
        .store
        .get_public_dashboard_by_dashboard_uid(ORG, "board")
        .await
        .unwrap()
        .unwrap();
    held.share = ShareType::Email;
    w.store.upsert_public_dashboard(held).await.unwrap();
    let (st, _) = read_panel(&w, &token).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call("GET", &format!("/api/public/dashboards/{token}"), None, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// Every route that works inside one Organization.
const ORG_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/org"),
    ("GET", "/api/datasources"),
    ("POST", "/api/datasources"),
    ("GET", "/api/datasources/uid/d"),
    ("PUT", "/api/datasources/uid/d"),
    ("DELETE", "/api/datasources/uid/d"),
    ("GET", "/api/datasources/uid/d/health"),
    ("GET", "/api/datasources/uid/d/resources/variable"),
    ("GET", "/api/folders"),
    ("POST", "/api/folders"),
    ("GET", "/api/folders/f"),
    ("DELETE", "/api/folders/f"),
    ("POST", "/api/dashboards/db"),
    ("GET", "/api/dashboards/uid/b"),
    ("DELETE", "/api/dashboards/uid/b"),
    ("GET", "/api/dashboards/uid/b/versions"),
    ("GET", "/api/dashboards/b/render?panel=1"),
    ("GET", "/api/search"),
    ("POST", "/api/ds/query"),
    ("GET", "/api/dashboards/uid/b/public-dashboards"),
    ("POST", "/api/dashboards/uid/b/public-dashboards"),
    ("PATCH", "/api/dashboards/uid/b/public-dashboards/p"),
    ("GET", "/odata/v4/Dashboards"),
    ("GET", "/odata/v4/Allowed(operation='read')"),
    ("GET", "/odata/v4/Dashboards('b')/Versions"),
    ("POST", "/odata/v4/Plan"),
    ("POST", "/odata/v4/Dashboards('b')/Export"),
    ("POST", "/odata/v4/$batch"),
];

fn admin_basic() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("admin:{ADMIN_PASSWORD}"))
    )
}

fn request(method: &str, uri: &str, auth: Option<&str>, org: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    if let Some(org) = org {
        req = req.header("x-grafana-org-id", org);
    }
    req.header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap()
}

#[tokio::test]
async fn a_request_without_an_organization_is_refused_on_every_route_of_one() {
    let w = mounted(world(), |state| {
        state.with_server_admin("admin", ADMIN_PASSWORD).unwrap()
    });
    let admin = admin_basic();
    for auth in [None, Some(admin.as_str())] {
        for (method, uri) in ORG_ROUTES {
            let (st, body) = w.send(request(method, uri, auth, None)).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{method} {uri}: {body}");
            assert!(!body.to_string().contains("Main Org."), "{method} {uri}");
        }
    }
    // Named, the same caller is served.
    let (st, body) = w.send(request("GET", "/api/org", Some(&admin), Some("1"))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Main Org.");
}

#[tokio::test]
async fn the_only_organization_of_a_verified_caller_is_theirs_and_several_must_be_named() {
    let w = mounted(world(), |state| {
        state.with_subject_resolver(|headers| {
            headers.contains_key("x-test-both").then(|| {
                Subject::authenticated("viewer", vec![ORG, OTHER_ORG], Vec::new())
            })
        })
    });
    let (st, body) = w.call("GET", "/api/org", Some("viewer"), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    let both = |org: Option<&str>| {
        let mut req = request("GET", "/api/org", None, org);
        req.headers_mut().insert("x-test-both", "1".parse().unwrap());
        req
    };
    let (st, body) = w.send(both(None)).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.to_string().contains("X-Grafana-Org-Id"), "{body}");
    let (st, body) = w.send(both(Some(&ORG.to_string()))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_installation_routes_and_the_dashboard_link_name_their_own_organization() {
    let w = mounted(world(), |state| {
        state.with_server_admin("admin", ADMIN_PASSWORD).unwrap()
    });
    let admin = admin_basic();
    let req = Request::builder()
        .method("POST")
        .uri("/api/orgs")
        .header("authorization", &admin)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"name":"doprava"}"#))
        .unwrap();
    let (st, body) = w.send(req).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = w
        .send(request("GET", "/api/orgs/name/doprava", Some(&admin), None))
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    w.add_dashboard(dashboard("linked", "prom"), None).await;
    let (st, _) = w.send(request("GET", "/d/linked", Some(&admin), None)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .send(request("GET", "/d/linked?orgId=1", Some(&admin), None))
        .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn an_unsigned_call_to_a_path_daiya_does_not_serve_is_401() {
    let w = world();
    for (method, uri) in [("POST", "/mcp"), ("GET", "/mcp/tools"), ("POST", "/mcp/call")] {
        let (st, _) = w.call(method, uri, None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{method} {uri}");
        let (st, _) = w.call(method, uri, Some("viewer"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}
