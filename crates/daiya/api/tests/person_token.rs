#![forbid(unsafe_code)]
//! A person's token on a panel read (the Portal's dashboards, G-4): the Portal sends it in
//! `x-daiya-person-token` beside its server-administrator credential, already exchanged for the
//! one Endpoint the panel reads. daiya hands it to that Endpoint's datasource and to no other,
//! ignores it on any other credential, and keys its cache on the person, so one person's frames
//! never serve another's. Repeated `var-<name>` values are formatted as Grafana's Prometheus
//! datasource formats them.

mod common;

use axum::Router;
use base64::Engine as _;
use common::{FlakyStore, ORG, bindings, loopback, prometheus, tokens};
use daiya_api::{
    DaiyaState, PERSON_TOKEN_HEADER, READ_AS_HEADER, READ_AS_PUBLIC, api_router, format_picked,
};
use daiya_model::{InstallationId, InstanceId, PolicyAuthorizer, VariableModel};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ADMIN: &str = "grafana_admin";
const PASSWORD: &str = "a-long-enough-server-admin-password";

/// An unsigned JWT for `sub` with `scope`; daiya reads it, the gateway would verify it.
fn jwt(sub: &str, scope: &str) -> String {
    let part = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.signature",
        part(json!({ "alg": "RS256", "typ": "JWT" })),
        part(json!({ "sub": sub, "scope": format!("openid {scope}") }))
    )
}

/// An NGSI-LD Endpoint answering each person with their own reading: `pm25` 1 for a request
/// carrying `pia`'s token, 2 for `ole`'s, 0 for anything else. Logs each request head.
async fn gateway(pia: String, ole: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            let value = if head.contains(&pia) {
                1
            } else if head.contains(&ole) {
                2
            } else {
                0
            };
            log.lock().push(head);
            let body = json!([{
                "id": "urn:ngsi-ld:AirQualityObserved:kallio",
                "type": "AirQualityObserved",
                "pm25": { "type": "Property", "value": value }
            }])
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), seen)
}

struct World {
    app: Router,
}

impl World {
    fn new() -> Self {
        let mut state = DaiyaState::new(
            InstallationId::from_u128(1),
            ORG,
            InstanceId::from_u128(1),
            Arc::new(FlakyStore::default()),
            Arc::new(PolicyAuthorizer::new(bindings())),
        )
        .with_tokens(tokens())
        .with_server_admin(ADMIN, PASSWORD)
        .unwrap()
        .with_person_tokens();
        state.outbound_policy = loopback();
        let app = api_router(
            state,
            Mount::new("daiya", "https", "x.example", "").unwrap(),
        );
        Self { app }
    }

    /// `method uri` with `authorization` and, when given, the person's token; with none, the
    /// read is for the public ([`READ_AS_HEADER`]), as the Portal sends it.
    async fn send(
        &self,
        method: &str,
        uri: &str,
        authorization: &str,
        person: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let header = match person {
            Some(token) => (PERSON_TOKEN_HEADER, token),
            None => (READ_AS_HEADER, READ_AS_PUBLIC),
        };
        self.send_with(method, uri, authorization, &[header], body)
            .await
    }

    /// `method uri` with `authorization` and exactly `headers`.
    async fn send_with(
        &self,
        method: &str,
        uri: &str,
        authorization: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                axum::body::Body::from(b.to_string())
            }
            None => axum::body::Body::empty(),
        };
        let resp = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())),
        )
    }

    async fn ok(&self, method: &str, uri: &str, body: Value) {
        let (status, answer) = self
            .send(method, uri, "Bearer admin", None, Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "{uri}: {answer}");
    }

    /// The panel as the Portal reads it: server administrator, for a person or the public.
    async fn panel(&self, uid: &str, person: Option<&str>) -> (StatusCode, Value) {
        self.send(
            "GET",
            &format!("/api/dashboards/{uid}/render?panel=1&from=1700000000000&to=1700000600000"),
            &admin_basic(),
            person,
            None,
        )
        .await
    }

    /// A datasource `uid` reading Endpoint `slug` at `base`, with or without pass-thru, and a
    /// dashboard `uid` whose one table panel reads it.
    async fn endpoint_board(&self, uid: &str, base: &str, slug: &str, pass_thru: bool) {
        self.ok(
            "POST",
            "/api/datasources",
            json!({
                "uid": uid, "name": uid, "type": "ngsild-grafana-datasource",
                "url": format!("{base}/api/endpoint/{slug}"),
                "jsonData": { "oauthPassThru": pass_thru }
            }),
        )
        .await;
        self.ok(
            "POST",
            "/api/dashboards/db",
            json!({ "overwrite": true, "dashboard": {
                "uid": uid, "title": uid, "schemaVersion": 42,
                "time": { "from": "now-1h", "to": "now" },
                "panels": [{
                    "id": 1, "type": "table", "title": "Air",
                    "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
                    "datasource": { "type": "ngsild-grafana-datasource", "uid": uid },
                    "targets": [{ "refId": "A", "datasource": { "type": "ngsild-grafana-datasource", "uid": uid },
                                  "queryType": "entity", "entityType": "AirQualityObserved", "attributes": ["pm25"] }]
                }]
            }}),
        )
        .await;
    }
}

/// The server administrator's `Authorization`, as the Portal sends it.
fn admin_basic() -> String {
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{ADMIN}:{PASSWORD}"));
    format!("Basic {basic}")
}

/// The pm25 values of a rendered panel.
fn readings(view: &Value) -> Vec<Value> {
    view["frameSets"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|set| set["frames"].as_array().cloned().unwrap_or_default())
        .flat_map(|frame| frame["fields"].as_array().cloned().unwrap_or_default())
        .filter(|field| field["name"] == "pm25")
        .flat_map(|field| field["values"].as_array().cloned().unwrap_or_default())
        .collect()
}

fn bearer_of(head: &str) -> Option<String> {
    head.lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("authorization:")
                .map(|_| line)
        })
        .map(|line| line.split_once(':').unwrap().1.trim().to_owned())
}

#[tokio::test]
async fn the_endpoint_gets_the_persons_token_and_without_it_none() {
    let pia = jwt("sub-pia", "endpoint:closedslug");
    let (base, seen) = gateway(pia.clone(), "nobody".into()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;
    w.endpoint_board("plain", &base, "closedslug", false).await;

    let (status, view) = w.panel("closed", Some(&pia)).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(readings(&view), [json!(1)], "{view}");
    let heads = std::mem::take(&mut *seen.lock());
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!(bearer_of(&heads[0]), Some(format!("Bearer {pia}")));
    assert!(
        !view.to_string().contains(&pia),
        "the answer carries the token"
    );

    // The same read through the server-admin channel with no person carries no Authorization.
    let (status, view) = w.panel("plain", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let heads = std::mem::take(&mut *seen.lock());
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!(bearer_of(&heads[0]), None, "{}", heads[0]);
}

#[tokio::test]
async fn a_token_for_another_endpoint_or_on_another_credential_goes_nowhere() {
    let elsewhere = jwt("sub-pia", "endpoint:otherslug");
    let (base, seen) = gateway(elsewhere.clone(), "nobody".into()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;

    let (_, view) = w.panel("closed", Some(&elsewhere)).await;
    assert!(readings(&view).is_empty(), "{view}");
    assert!(
        seen.lock().is_empty(),
        "the Endpoint was asked: {:?}",
        seen.lock()
    );

    // On an API credential that is not the server administrator's the header is ignored.
    let scoped = jwt("sub-pia", "endpoint:closedslug");
    let (_, view) = w
        .send(
            "GET",
            "/api/dashboards/closed/render?panel=1&from=1700000000000&to=1700000600000",
            "Bearer admin",
            Some(&scoped),
            None,
        )
        .await;
    assert!(readings(&view).is_empty(), "{view}");
    assert!(
        seen.lock().iter().all(|head| !head.contains(&scoped)),
        "{:?}",
        seen.lock()
    );
}

#[tokio::test]
async fn two_persons_reading_one_panel_never_get_each_others_frames() {
    let pia = jwt("sub-pia", "endpoint:closedslug");
    let ole = jwt("sub-ole", "endpoint:closedslug");
    let (base, seen) = gateway(pia.clone(), ole.clone()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;

    let (_, first) = w.panel("closed", Some(&pia)).await;
    let (_, second) = w.panel("closed", Some(&ole)).await;
    let (_, again) = w.panel("closed", Some(&pia)).await;
    assert_eq!(readings(&first), [json!(1)], "{first}");
    assert_eq!(readings(&second), [json!(2)], "{second}");
    assert_eq!(readings(&again), [json!(1)], "{again}");
    // pia's second read is served from the cache, ole's never was.
    assert_eq!(seen.lock().len(), 2, "{:?}", seen.lock());
}

#[tokio::test]
async fn a_panel_request_without_a_person_token_is_refused() {
    let pia = jwt("sub-pia", "endpoint:closedslug");
    let (base, seen) = gateway(pia, "nobody".into()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;
    let ds_query = json!({ "from": "1700000000000", "to": "1700000600000", "queries": [{
        "refId": "A", "datasource": { "type": "ngsild-grafana-datasource", "uid": "closed" },
        "queryType": "entity", "entityType": "AirQualityObserved", "attributes": ["pm25"]
    }]});
    let reads = [
        (
            "GET",
            "/api/dashboards/closed/render?panel=1&from=1700000000000&to=1700000600000",
            None,
        ),
        ("POST", "/api/ds/query", Some(ds_query)),
        (
            "GET",
            "/api/datasources/uid/closed/resources/entities",
            None,
        ),
    ];
    // No header, an empty person token, and a read-as value that is not `public`.
    let missing: [&[(&str, &str)]; 3] = [
        &[],
        &[(PERSON_TOKEN_HEADER, " ")],
        &[(READ_AS_HEADER, "admin")],
    ];
    for (method, uri, body) in &reads {
        for headers in missing {
            let (status, answer) = w
                .send_with(method, uri, &admin_basic(), headers, body.clone())
                .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} {headers:?}: {answer}");
            assert!(answer.to_string().contains(PERSON_TOKEN_HEADER), "{answer}");
        }
    }
    assert!(seen.lock().is_empty(), "the Endpoint was asked: {:?}", seen.lock());

    // The server-administration routes read no data and stay open to the credential.
    for uri in ["/api/datasources", "/api/search", "/api/dashboards/uid/closed"] {
        let (status, answer) = w.send_with("GET", uri, &admin_basic(), &[], None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {answer}");
    }
}

#[tokio::test]
async fn the_cache_never_keys_on_an_unverified_sub() {
    // Two tokens claiming one `sub`: daiya verifies neither, so it must not take them for one person.
    let pia = jwt("sub-pia", "endpoint:closedslug");
    let forged = pia.replace(".signature", ".forged");
    let (base, seen) = gateway(pia.clone(), forged.clone()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;

    let (_, first) = w.panel("closed", Some(&pia)).await;
    let (_, second) = w.panel("closed", Some(&forged)).await;
    assert_eq!(readings(&first), [json!(1)], "{first}");
    assert_eq!(readings(&second), [json!(2)], "{second}");
    assert_eq!(seen.lock().len(), 2, "{:?}", seen.lock());
}

#[tokio::test]
async fn a_public_dashboard_reads_as_public_only() {
    let pia = jwt("sub-pia", "endpoint:closedslug");
    let (base, seen) = gateway(pia.clone(), "nobody".into()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;
    let (status, created) = w
        .send(
            "POST",
            "/api/dashboards/uid/closed/public-dashboards",
            "Bearer admin",
            None,
            Some(json!({ "uid": "pd-closed", "isEnabled": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = created["accessToken"].as_str().unwrap().to_owned();

    // Even beside the server-admin credential and a person's token, the public door reads as the
    // public: the Endpoint sees no Authorization and answers its public reading (0).
    let (status, view) = w
        .send_with(
            "POST",
            &format!("/api/public/dashboards/{token}/panels/1/query"),
            &admin_basic(),
            &[(PERSON_TOKEN_HEADER, &pia)],
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(!view.to_string().contains(&pia), "{view}");
    let heads = seen.lock().clone();
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!(bearer_of(&heads[0]), None, "{}", heads[0]);
}

fn variable(value: Value) -> VariableModel {
    serde_json::from_value(value).unwrap()
}

#[test]
fn picked_values_are_formatted_as_grafanas_prometheus_formats_them() {
    let k6 = variable(json!({
        "name": "testid", "type": "query", "multi": true, "includeAll": true, "allValue": ".*",
        "options": [{ "text": "a.b", "value": "a.b" }, { "text": "c", "value": "c" }]
    }));
    let picked = |values: &[&str]| {
        let values: Vec<String> = values.iter().map(|v| (*v).to_owned()).collect();
        format_picked(Some(&k6), &values)
    };
    assert_eq!(picked(&["a.b", "c"]), r"(a\.b|c)");
    assert_eq!(picked(&["a.b"]), r"a\.b");
    assert_eq!(picked(&["$__all"]), ".*");
    let no_all_value = variable(json!({
        "name": "s", "type": "custom", "multi": true, "includeAll": true,
        "options": [{ "text": "All", "value": "$__all" }, { "text": "x|y", "value": "x|y" }, { "text": "z", "value": "z" }]
    }));
    assert_eq!(
        format_picked(Some(&no_all_value), &["$__all".to_owned()]),
        r"(x\|y|z)"
    );
    let single = variable(json!({ "name": "job", "type": "custom" }));
    assert_eq!(format_picked(Some(&single), &["a.b".to_owned()]), "a.b");
}

#[tokio::test]
async fn a_multi_value_variable_reaches_prometheus_as_a_regex_alternation() {
    let (url, seen) = prometheus().await;
    let w = World::new();
    w.ok(
        "POST",
        "/api/datasources",
        json!({ "uid": "prom", "name": "Prom", "type": "prometheus", "url": url }),
    )
    .await;
    w.ok(
        "POST",
        "/api/dashboards/db",
        json!({ "overwrite": true, "dashboard": {
            "uid": "k6", "title": "k6", "schemaVersion": 42,
            "time": { "from": "now-1h", "to": "now" },
            "templating": { "list": [{
                "name": "testid", "type": "query", "multi": true, "includeAll": true, "allValue": ".*",
                "datasource": { "type": "prometheus", "uid": "prom" }, "current": {}
            }]},
            "panels": [{
                "id": 1, "type": "timeseries", "title": "Requests",
                "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
                "datasource": { "type": "prometheus", "uid": "prom" },
                "targets": [{ "refId": "A", "datasource": { "type": "prometheus", "uid": "prom" },
                              "expr": "sum(rate(k6_http_reqs_total{testid=~\"$testid\"}[30s]))" }]
            }]
        }}),
    )
    .await;
    for query in ["var-testid=r.1&var-testid=r2", "var-testid=%24__all"] {
        let (status, view) = w
            .send(
                "GET",
                &format!(
                    "/api/dashboards/k6/render?panel=1&from=1700000000000&to=1700000600000&{query}"
                ),
                &admin_basic(),
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{view}");
    }
    let seen = seen.lock().clone();
    let asked: Vec<String> = seen
        .iter()
        .filter(|head| head.contains("query_range"))
        .map(|head| {
            // The query rides in the address or in the form body.
            let target = head.split_whitespace().nth(1).unwrap_or_default();
            let address = target.split_once('?').map_or("", |(_, q)| q);
            let body = head.split_once("\r\n\r\n").map_or("", |(_, b)| b);
            [address, body]
                .iter()
                .flat_map(|part| url::form_urlencoded::parse(part.as_bytes()))
                .find(|(k, _)| k == "query")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        asked,
        [
            r#"sum(rate(k6_http_reqs_total{testid=~"(r\\.1|r2)"}[30s]))"#,
            r#"sum(rate(k6_http_reqs_total{testid=~".*"}[30s]))"#,
        ],
        "{seen:?}"
    );
}

/// A members-only Endpoint: answers 401 to every request and logs each request head.
async fn members_only_gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            log.lock()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let body = r#"{"type":"about:blank","title":"Unauthorized","status":401}"#;
            let resp = format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/problem+json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), seen)
}

#[tokio::test]
async fn a_public_endpoint_panel_renders_with_no_person_token_and_no_authorization() {
    let (base, seen) = gateway("nobody-a".into(), "nobody-b".into()).await;
    let w = World::new();
    // oauthPassThru on, as the Portal provisions every Endpoint datasource.
    w.endpoint_board("public", &base, "publicslug", true).await;

    let (status, view) = w.panel("public", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(readings(&view), [json!(0)], "{view}");
    let heads = std::mem::take(&mut *seen.lock());
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!(bearer_of(&heads[0]), None, "{}", heads[0]);
}

#[tokio::test]
async fn a_members_only_endpoint_read_with_no_token_goes_out_anonymously_and_its_401_is_the_error()
{
    let (base, seen) = members_only_gateway().await;
    let w = World::new();
    w.endpoint_board("members", &base, "membersslug", true)
        .await;

    let (status, view) = w.panel("members", None).await;
    assert!(readings(&view).is_empty(), "{view}");
    assert_ne!(status, StatusCode::OK, "{view}");
    assert!(
        view.to_string().contains("upstream status 401"),
        "{status} {view}"
    );
    let heads = std::mem::take(&mut *seen.lock());
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!(bearer_of(&heads[0]), None, "{}", heads[0]);
}

#[tokio::test]
async fn an_anonymous_read_and_a_persons_read_of_one_panel_never_share_a_cache_entry() {
    let pia = jwt("sub-pia", "endpoint:closedslug");
    let (base, seen) = gateway(pia.clone(), "nobody".into()).await;
    let w = World::new();
    w.endpoint_board("closed", &base, "closedslug", true).await;

    let (_, anonymous) = w.panel("closed", None).await;
    let (_, person) = w.panel("closed", Some(&pia)).await;
    let (_, anonymous_again) = w.panel("closed", None).await;
    let (_, person_again) = w.panel("closed", Some(&pia)).await;
    assert_eq!(readings(&anonymous), [json!(0)], "{anonymous}");
    assert_eq!(readings(&person), [json!(1)], "{person}");
    assert_eq!(readings(&anonymous_again), [json!(0)], "{anonymous_again}");
    assert_eq!(readings(&person_again), [json!(1)], "{person_again}");
    // One upstream read each; the repeats came from two separate cache entries.
    let heads = seen.lock().clone();
    assert_eq!(heads.len(), 2, "{heads:?}");
    assert_eq!(bearer_of(&heads[0]), None, "{}", heads[0]);
    assert_eq!(bearer_of(&heads[1]), Some(format!("Bearer {pia}")));
}
