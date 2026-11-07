// SPDX-License-Identifier: EUPL-1.2
//! Equivalence across surfaces (surfaces-test-plan section 3, T-3522): over
//! five views of one corpus (`tests/fixtures/equivalence.jsonld`, 200
//! Entities in three types with Relationships between them), the same read
//! answers the same Entities on the NGSI-LD view root, OData, MCP and the
//! WebSocket channel, and counts and pages agree.
//!
//! - NGSI-LD `/x/views/{v}/ngsi-ld/v1/entities` in the normalized form is the
//!   reference: an Entity is its id and each Attribute's value (a
//!   Relationship's object).
//! - OData `/x/views/{v}/odata/v4/{Type}`: every member of a row against
//!   the reference, where a Relationship to a type the view serves is a
//!   navigation property, absent from a row until expanded, and one to a
//!   type it does not serve is the object URI (`odata-linkml-mapping.md`
//!   rows R1 and R5).
//! - MCP `tools/call` of `get_entity` and `query_entities`.
//! - WebSocket `/x/views/{v}/ws`: a Subscription made before the corpus is
//!   upserted is notified the Entities the view read gives afterwards.
//!
//! No surface answers wider than the NGSI-LD view: every comparison is an
//! equality, so an extra Entity or Attribute on any of them is red.
#![cfg(feature = "full")]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tower::Service;

#[path = "common/ws_client.rs"]
mod ws_client;
use ws_client::Socket;

/// The Tenant the views serve.
const TENANT: &str = "eq";
const CORPUS: &str = include_str!("fixtures/equivalence.jsonld");

/// The Tenant's `LinkML` schema: the three types under the core context's
/// default vocabulary, `location` the core GeoProperty, Relationships to
/// Building.
const SCHEMA: &str = r#"{
  "id": "https://example.org/equivalence", "name": "equivalence",
  "prefixes": {"d": {"prefix_prefix": "d", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
  "default_prefix": "d",
  "classes": {
    "Building": {"slots": ["name", "floors", "location"]},
    "Sensor": {"slots": ["name", "temperature", "location", "building"]},
    "Vehicle": {"slots": ["name", "speed", "location", "depot"]}
  },
  "slots": {
    "name": {"range": "string"}, "floors": {"range": "integer"},
    "temperature": {"range": "integer"}, "speed": {"range": "integer"},
    "location": {"slot_uri": "https://uri.etsi.org/ngsi-ld/location",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "building": {"range": "Building"}, "depot": {"range": "Building"}
  }
}"#;

/// One of the five views: its id, the types it selects, and per type a `q`
/// with its OData `$filter`.
struct View {
    id: &'static str,
    types: &'static [&'static str],
    doc: Value,
    /// How many corpus Entities it serves, counted from the fixture by hand.
    serves: usize,
}

/// A `q` per type and the same condition as an OData `$filter`.
const QUERIES: [(&str, &str, &str); 3] = [
    ("Building", "floors>=6", "floors ge 6"),
    ("Sensor", "temperature>60", "temperature gt 60"),
    ("Vehicle", "speed<30", "speed lt 30"),
];

fn views() -> Vec<View> {
    let every = |types: &[&str], attributes: &[&str]| -> Value {
        json!(types
            .iter()
            .map(|t| json!({"types": [t], "attributes": attributes}))
            .collect::<Vec<_>>())
    };
    let all_attrs = [
        "name",
        "floors",
        "temperature",
        "speed",
        "location",
        "building",
        "depot",
    ];
    let three: &[&str] = &["Building", "Sensor", "Vehicle"];
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let view = |id: &'static str,
                types: &'static [&'static str],
                serves: usize,
                extra: Value,
                rules: Value| {
        let mut doc = json!({"id": id, "tenant": TENANT, "selector": types,
                             "rules": rules, "operations": ops});
        for (k, v) in extra.as_object().into_iter().flatten() {
            doc[k] = v.clone();
        }
        View {
            id,
            types,
            doc,
            serves,
        }
    };
    vec![
        view("all", three, 200, json!({}), every(three, &all_attrs)),
        view(
            "types-only",
            &["Building", "Sensor"],
            140,
            json!({}),
            every(&["Building", "Sensor"], &all_attrs),
        ),
        view(
            "hidden-attr",
            three,
            200,
            json!({}),
            every(
                three,
                &[
                    "floors",
                    "temperature",
                    "speed",
                    "location",
                    "building",
                    "depot",
                ],
            ),
        ),
        view(
            "q-narrowed",
            &["Sensor"],
            49,
            json!({"q": "temperature>50"}),
            every(&["Sensor"], &all_attrs),
        ),
        view(
            "scope-geo",
            &["Sensor", "Vehicle"],
            40,
            json!({"scopeQ": "/city/north", "geoQ": {"georel": "within", "geometry": "Polygon",
                   "coordinates": [[[13.995, 49.9], [14.045, 49.9], [14.045, 51.0],
                                    [13.995, 51.0], [13.995, 49.9]]]}}),
            every(&["Sensor", "Vehicle"], &all_attrs),
        ),
    ]
}

/// The broker with every surface of a `full` build over the built-in
/// store, the five views and the schema loaded, and no Entity yet; the
/// view table's reload handle, held as long as the router serves.
async fn broker() -> (Router, antares_ops::views::Reload) {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let mut state = AppState::new("equivalence".into())
        .with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())));
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        antares_ops::mount(antares_odata::OData),
        antares_ops::mount(antares_mcp::Mcp),
        antares_ops::mount(antares_ws::Ws::process()),
        antares_ops::mount(antares_ws::ViewWs::process()),
    ];
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    let reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    let app = antares_api::router(state);
    for v in views() {
        let (status, _, body) =
            send(&app, "PUT", &format!("/q/views/{}", v.id), Some(&v.doc)).await;
        assert!(status.is_success(), "view {}: {status} {body}", v.id);
    }
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema");
    let (status, _, body) = send(&app, "PUT", &format!("/q/schemas/{TENANT}"), Some(&schema)).await;
    assert!(status.is_success(), "schema: {status} {body}");
    (app, reload)
}

/// Upsert the corpus on the Tenant root.
async fn load(app: &Router) {
    let corpus: Value = serde_json::from_str(CORPUS).expect("the corpus");
    assert_eq!(corpus.as_array().map(Vec::len), Some(200));
    let (status, _, body) = send(
        app,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert",
        Some(&corpus),
    )
    .await;
    assert!(status.is_success(), "upsert: {status} {body}");
}

/// One request; the status, headers and JSON body (`Null` when not JSON).
async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (StatusCode, HeaderMap, Value) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost")
        .header("NGSILD-Tenant", TENANT);
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", text.len());
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("request"))
        .await;
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// An Entity as the comparison reads it: id, and each Attribute's value.
type Entity = (String, BTreeMap<String, Value>);

/// A normalized NGSI-LD Entity.
fn ngsi(e: &Value) -> Entity {
    let mut attrs = BTreeMap::new();
    for (k, v) in e.as_object().into_iter().flatten() {
        let value = match v.get("type").and_then(Value::as_str) {
            Some("Property" | "GeoProperty") => v.get("value"),
            Some("Relationship") => v.get("object"),
            _ => None,
        };
        if let Some(value) = value {
            attrs.insert(k.clone(), value.clone());
        }
    }
    (text(e, "id"), attrs)
}

/// What an OData row of `e` carries in a view selecting `types`: a
/// Relationship to a visible type is a navigation property, absent until
/// expanded (mapping row R1); to a type the view does not serve, the object
/// URI as a string (row R5).
fn as_odata(e: &Entity, types: &[&str]) -> Entity {
    let navigates = types.contains(&"Building");
    let kept =
        e.1.iter()
            .filter(|(k, _)| !(navigates && matches!(k.as_str(), "building" | "depot")))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    (e.0.clone(), kept)
}

/// An OData row: its id and every member that is not an annotation.
fn odata_row(row: &Value) -> Entity {
    let attrs = row
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| !k.contains('@') && !matches!(k.as_str(), "id" | "_types" | "scope"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (text(row, "id"), attrs)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn rows(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The NGSI-LD view's Query Entities with `query`, every page.
async fn ngsi_query(app: &Router, view: &str, query: &str) -> Vec<Value> {
    let uri = format!("/x/views/{view}/ngsi-ld/v1/entities?{query}");
    let (status, _, body) = send(app, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body.as_array().cloned().unwrap_or_default()
}

/// The OData EntitySet `set` with `query`, following `@odata.nextLink`.
async fn odata_query(app: &Router, view: &str, set: &str, query: &str) -> Vec<Value> {
    let mut uri = format!("/x/views/{view}/odata/v4/{set}?{query}");
    let mut out = Vec::new();
    loop {
        let (status, _, body) = send(app, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        out.extend(rows(&body, "value"));
        match body.get("@odata.nextLink").and_then(Value::as_str) {
            Some(next) => {
                let path = next.find("/x/views/").map_or(next, |i| &next[i..]);
                uri = path.to_owned();
            }
            None => return out,
        }
    }
}

/// One MCP `tools/call` on `view`; the JSON-RPC answer.
async fn mcp(app: &Router, view: &str, tool: &str, args: Value) -> Value {
    const MODERN: &str = "2026-07-28";
    let params = json!({"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "equivalence", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    }});
    let body =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}).to_string();
    let req = Request::post(format!("/x/views/{view}/mcp"))
        .header("Host", "localhost")
        .header("NGSILD-Tenant", TENANT)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", MODERN)
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", tool)
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let Ok(res) = app.clone().call(req).await;
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// `query_entities` with `args`, page by page with `limit`; the pages.
async fn mcp_pages(app: &Router, view: &str, args: &Value, limit: u64) -> Vec<Vec<Value>> {
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut a = args.clone();
        a["limit"] = json!(limit);
        if let Some(c) = &cursor {
            a["cursor"] = json!(c);
        }
        let answer = mcp(app, view, "query_entities", a).await;
        let out = answer
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or_else(|| panic!("{view} query_entities {args}: {answer}"));
        pages.push(rows(&out, "entities"));
        cursor = out
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            return pages;
        }
    }
}

fn ids(entities: &[Entity]) -> Vec<&str> {
    entities.iter().map(|(id, _)| id.as_str()).collect()
}

/// Read shapes by type, by `q` and paged, and by id, on every view: the
/// four surfaces answer the same Entities; the WebSocket Subscription made
/// before the corpus is upserted is notified exactly the view's Entities.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_read_returns_the_same_entities_on_every_surface() {
    let (app, _reload) = broker().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });

    // a socket per view, subscribed before anything is stored
    let mut sockets = Vec::new();
    for v in views() {
        let types: Vec<Value> = v.types.iter().map(|t| json!({"type": t})).collect();
        let id = v.id;
        let socket = tokio::task::spawn_blocking(move || {
            let mut s = Socket::open(
                port,
                &format!("/x/views/{id}/ws"),
                &[("NGSILD-Tenant".to_owned(), TENANT.to_owned())],
                std::time::Duration::from_secs(2),
            )
            .unwrap_or_else(|e| panic!("{id}: {e}"));
            let welcome = s.next().unwrap_or_default();
            assert_eq!(welcome["metadata"]["x-op"], "x-welcome", "{id}: {welcome}");
            s.send(&json!({"metadata": {"x-op": "x-subscribe", "x-id": id},
                           "body": {"type": "Subscription", "entities": types}}))
                .unwrap_or_else(|e| panic!("{id}: {e}"));
            let made = s.next().unwrap_or_default();
            assert_eq!(made["metadata"]["x-status"], 201, "{id}: {made}");
            s
        })
        .await
        .expect("subscribe");
        sockets.push((id, socket));
    }
    load(&app).await;

    let mut failures = Vec::new();
    for v in views() {
        // the reference: the view's Entities of each type
        let mut by_type: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for t in v.types {
            by_type.insert(
                t,
                ngsi_query(&app, v.id, &format!("type={t}&limit=1000")).await,
            );
        }
        let everything: Vec<Entity> = by_type.values().flatten().map(ngsi).collect();
        assert_eq!(everything.len(), v.serves, "{}: what the view serves", v.id);
        if v.id == "hidden-attr" {
            assert!(
                everything.iter().all(|(_, a)| !a.contains_key("name")),
                "the hidden Attribute is shown"
            );
        }

        for t in v.types {
            let reference = &by_type[t];
            for (shape, ngsi_q, odata_q, mcp_args) in [
                (
                    "by type",
                    String::new(),
                    String::new(),
                    json!({"type": [t]}),
                ),
                {
                    let (_, q, f) = QUERIES.iter().find(|(qt, _, _)| qt == t).expect("a q");
                    (
                        "by q",
                        format!("&q={}", enc(q)),
                        format!("&$filter={}", enc(f)),
                        json!({"type": [t], "q": q}),
                    )
                },
            ] {
                let want: Vec<Entity> = if ngsi_q.is_empty() {
                    reference.iter().map(ngsi).collect()
                } else {
                    ngsi_query(&app, v.id, &format!("type={t}&limit=1000{ngsi_q}"))
                        .await
                        .iter()
                        .map(ngsi)
                        .collect()
                };
                let props: Vec<Entity> = want.iter().map(|e| as_odata(e, v.types)).collect();
                let odata: Vec<Entity> = odata_query(&app, v.id, t, &format!("$top=1000{odata_q}"))
                    .await
                    .iter()
                    .map(odata_row)
                    .collect();
                if odata != props {
                    failures.push(format!(
                        "{} {t} {shape}: OData {odata:?} != {props:?}",
                        v.id
                    ));
                }
                let mcp: Vec<Entity> = mcp_pages(&app, v.id, &mcp_args, 1000)
                    .await
                    .concat()
                    .iter()
                    .map(ngsi)
                    .collect();
                if mcp != want {
                    failures.push(format!(
                        "{} {t} {shape}: MCP {:?} != {:?}",
                        v.id,
                        ids(&mcp),
                        ids(&want)
                    ));
                }
            }
            // paged: the third page of seven
            let want: Vec<Entity> = ngsi_query(&app, v.id, &format!("type={t}&limit=7&offset=14"))
                .await
                .iter()
                .map(ngsi)
                .collect();
            let odata: Vec<String> = odata_query(&app, v.id, t, "$top=7&$skip=14")
                .await
                .iter()
                .map(|r| text(r, "id"))
                .collect();
            let mcp: Vec<Entity> = mcp_pages(&app, v.id, &json!({"type": [t]}), 7)
                .await
                .get(2)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(ngsi)
                .collect();
            if odata != ids(&want) || mcp != want {
                failures.push(format!(
                    "{} {t} paged: NGSI-LD {:?}, OData {odata:?}, MCP {:?}",
                    v.id,
                    ids(&want),
                    ids(&mcp)
                ));
            }
        }

        // by id: one Entity of each corpus type, in the view or not
        for id in [
            "urn:ngsi-ld:Building:eq-007",
            "urn:ngsi-ld:Sensor:eq-010",
            "urn:ngsi-ld:Sensor:eq-011",
            "urn:ngsi-ld:Vehicle:eq-003",
        ] {
            let kind = id.split(':').nth(2).unwrap_or_default();
            let (status, _, body) = send(
                &app,
                "GET",
                &format!("/x/views/{}/ngsi-ld/v1/entities/{id}", v.id),
                None,
            )
            .await;
            let want = (status == StatusCode::OK).then(|| ngsi(&body));
            let (status, _, row) = send(
                &app,
                "GET",
                &format!("/x/views/{}/odata/v4/{kind}('{}')", v.id, enc(id)),
                None,
            )
            .await;
            let odata = (status == StatusCode::OK).then(|| odata_row(&row));
            let answer = mcp(&app, v.id, "get_entity", json!({"id": id})).await;
            let mcp = answer
                .pointer("/result/structuredContent/entity")
                .filter(|_| answer.pointer("/result/isError") != Some(&json!(true)))
                .map(ngsi);
            let props = want.as_ref().map(|e| as_odata(e, v.types));
            if odata != props || mcp != want {
                failures.push(format!(
                    "{} by id {id}: NGSI-LD {want:?}, OData {odata:?}, MCP {mcp:?}",
                    v.id
                ));
            }
        }

        // the WebSocket: every notified Entity, the last of each id
        let Some(pos) = sockets.iter().position(|(id, _)| *id == v.id) else {
            continue;
        };
        let (id, mut socket) = sockets.swap_remove(pos);
        let notified = tokio::task::spawn_blocking(move || {
            let mut seen: BTreeMap<String, Value> = BTreeMap::new();
            while let Some(m) = socket.next() {
                if m["metadata"]["x-op"] == "x-notification" {
                    for e in rows(&m["body"], "data") {
                        seen.insert(text(&e, "id"), e);
                    }
                }
            }
            seen
        })
        .await
        .expect("notifications");
        let mut ws: Vec<Entity> = notified.values().map(ngsi).collect();
        let mut want = everything.clone();
        ws.sort_by(|a, b| a.0.cmp(&b.0));
        want.sort_by(|a, b| a.0.cmp(&b.0));
        if ws != want {
            failures.push(format!(
                "{id} WebSocket: {} notified, {} read; first difference {:?}",
                ws.len(),
                want.len(),
                ws.iter().zip(&want).find(|(a, b)| a != b)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `NGSILD-Results-Count`, `@odata.count` and the MCP `total` are equal,
/// and paging through each surface yields the same ids in the same order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn count_and_paging_agree_on_every_surface() {
    let (app, _reload) = broker().await;
    load(&app).await;
    let mut failures = Vec::new();
    for v in views() {
        for t in v.types {
            let (status, headers, _) = send(
                &app,
                "GET",
                &format!(
                    "/x/views/{}/ngsi-ld/v1/entities?type={t}&count=true&limit=0",
                    v.id
                ),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{} {t}", v.id);
            let ngsi_count: Option<u64> = headers
                .get("NGSILD-Results-Count")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.parse().ok());
            let (_, _, body) = send(
                &app,
                "GET",
                &format!("/x/views/{}/odata/v4/{t}?$count=true&$top=0", v.id),
                None,
            )
            .await;
            let odata_count = body.get("@odata.count").and_then(Value::as_u64);
            let answer = mcp(
                &app,
                v.id,
                "query_entities",
                json!({"type": [t], "count": true, "limit": 1}),
            )
            .await;
            let mcp_count = answer
                .pointer("/result/structuredContent/total")
                .and_then(Value::as_u64);
            if ngsi_count.is_none() || ngsi_count != odata_count || ngsi_count != mcp_count {
                failures.push(format!(
                    "{} {t} count: NGSI-LD {ngsi_count:?}, OData {odata_count:?}, MCP {mcp_count:?}",
                    v.id
                ));
            }

            // pages of nine on each surface
            let mut ngsi_ids = Vec::new();
            for page in 0.. {
                let got =
                    ngsi_query(&app, v.id, &format!("type={t}&limit=9&offset={}", page * 9)).await;
                if got.is_empty() {
                    break;
                }
                ngsi_ids.extend(got.iter().map(|e| text(e, "id")));
            }
            let mut odata_ids = Vec::new();
            for page in 0.. {
                let got = odata_query(&app, v.id, t, &format!("$top=9&$skip={}", page * 9)).await;
                if got.is_empty() {
                    break;
                }
                odata_ids.extend(got.iter().map(|e| text(e, "id")));
            }
            let mcp_ids: Vec<String> = mcp_pages(&app, v.id, &json!({"type": [t]}), 9)
                .await
                .concat()
                .iter()
                .map(|e| text(e, "id"))
                .collect();
            let total = usize::try_from(ngsi_count.unwrap_or_default()).unwrap_or_default();
            if ngsi_ids.len() != total || odata_ids != ngsi_ids || mcp_ids != ngsi_ids {
                failures.push(format!(
                    "{} {t} paging: NGSI-LD {ngsi_ids:?}, OData {odata_ids:?}, MCP {mcp_ids:?}",
                    v.id
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `s` percent-encoded for a query or a key.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
