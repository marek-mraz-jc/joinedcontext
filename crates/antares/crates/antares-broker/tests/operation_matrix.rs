// SPDX-License-Identifier: EUPL-1.2
//! The operation matrix against the surfaces a `views` build composes
//! (surfaces-test-plan section 2, T-3500).
//!
//! The matrix rows are [`OperationId`]: `antares-ops/tests/operation_ids.rs`
//! proves its 69 ids, bindings and gate clauses are the rows of
//! `docs/analysis/operation-matrix.yaml`, so this file reads the yaml only
//! for what `OperationId` does not carry: the `non_operation_routes` and
//! each row's `write` flag.
//!
//! - Every route the NGSI-LD router serves is a matrix cell or a listed
//!   non-operation route, and every row the broker answers is reached by a
//!   route: a route added without a row is red.
//! - Every cell answers as stated, on the root and through the view root
//!   (ADR-0023) under view `all` (every operation) and view `restricted`
//!   (one type, one hidden Attribute, read rows only): a served cell asks
//!   the policy engine exactly once, with the row's gate clause; any other
//!   is refused (404, 405 or 422, ADR-0023 rule 4) before the engine is
//!   asked; and no answer through `restricted` carries the hidden
//!   Attribute.
//! - In an `odata` build, the OData column (T-3508): every row marked
//!   `odata: supported` is sent in its OData form through both views; a
//!   read asks the engine once with its clause, the service document and
//!   `$metadata` ask nothing, a write asks once under `all` and is 405
//!   before the engine under `restricted`, the forms of a later round
//!   are refused before the engine, and a supported row this column does
//!   not send is red.
//!
//! - In an `mcp` build, the MCP column (T-3656): `tools/list` through
//!   view `all` is exactly the tools the matrix's `mcp: supported` cells
//!   name, through `restricted` exactly those of the read rows; every
//!   tool is called (a write asked and confirmed) and asks the engine
//!   with its row's gate clause once, after the one probe that keeps a
//!   hidden tool unknown (MC3). A tool added without a row is red.
//! - In a `ws` build, the WebSocket column (T-3696): every `x-op` the
//!   envelope of `antares-ws` reads is the binding of a `supported`
//!   websocket cell or a `session_messages` entry, and back. Over a real
//!   socket on the Tenant root and view `all`, each op asks the engine
//!   with its row's gate clause once (plus Retrieve Subscription where the
//!   binding says it answers with the stored Subscription), a change is
//!   pushed as a notification asking nothing more, and through view
//!   `restricted` (reads only), an unknown `x-op` and `x-resume` are
//!   refused before the engine.
#![cfg(feature = "views")]

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use antares_ops::OperationId;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tower::Service;

const API: &str = "/ngsi-ld/v1";
const MATRIX: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/analysis/operation-matrix.yaml"
);
/// The Attribute view `restricted` hides.
const HIDDEN: &str = "secret";
/// The Tenant both views serve.
const TENANT: &str = "city";

/// Records the clause of every decision it is asked for, and allows.
#[derive(Clone, Default)]
struct Clauses(Arc<Mutex<Vec<&'static str>>>);

impl Clauses {
    fn take(&self) -> Vec<&'static str> {
        self.0
            .lock()
            .map(|mut seen| std::mem::take(&mut *seen))
            .unwrap_or_default()
    }
}

impl PolicyEngine for Clauses {
    fn name(&self) -> &str {
        "clauses"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if let Ok(mut seen) = self.0.lock() {
            seen.push(op.clause);
        }
        Box::pin(std::future::ready(Decision::Allow))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// The yaml's lines under the top-level key `section`.
fn section<'a>(text: &'a str, section: &str) -> impl Iterator<Item = &'a str> {
    let header = format!("{section}:");
    text.lines()
        .skip_while(move |l| *l != header)
        .skip(1)
        .take_while(|l| l.starts_with(' ') || l.starts_with('-'))
}

/// `non_operation_routes` that name methods and an NGSI-LD path: `(methods,
/// path)`, e.g. `(["PATCH", "PUT"], "/ngsi-ld/v1/entities")`.
fn non_operation_routes(text: &str) -> Vec<(Vec<String>, String)> {
    section(text, "non_operation_routes")
        .filter_map(|l| l.strip_prefix("- route: "))
        .filter_map(|r| r.split_once(' '))
        .filter(|(_, path)| path.starts_with(API))
        .map(|(methods, path)| {
            (
                methods.split('|').map(str::to_owned).collect(),
                path.trim().to_owned(),
            )
        })
        .collect()
}

/// The ids of the rows whose `write:` is true.
fn writes(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut row = None;
    for l in section(text, "operations") {
        if let Some(id) = l.strip_prefix("- id: ") {
            row = Some(id.trim().to_owned());
        } else if l == "  write: true" {
            out.extend(row.take());
        }
    }
    out
}

/// One request the walk sends: a route of the NGSI-LD router with the
/// query and body that carry it to its gate.
struct Cell {
    method: String,
    /// The route's template, below the API root.
    route: String,
    query: &'static str,
    body: Value,
}

impl Cell {
    fn path(&self) -> String {
        format!("{API}{}", concrete(&self.route))
    }

    fn op(&self) -> Option<OperationId> {
        let query = Some(self.query).filter(|q| !q.is_empty());
        OperationId::of(&self.method, &self.path(), query)
    }

    fn request(&self, root: &str) -> Request<Body> {
        let sep = if self.query.is_empty() { "" } else { "?" };
        let uri = format!("{root}{}{sep}{}", self.path(), self.query);
        let b = Request::builder().method(self.method.as_str()).uri(uri);
        if self.body.is_null() {
            return b.body(Body::empty()).expect("request");
        }
        let payload = self.body.to_string();
        b.header("Content-Type", "application/json")
            .header("Content-Length", payload.len())
            .body(Body::from(payload))
            .expect("request")
    }
}

/// A route's parameters filled in with values the handlers accept.
fn concrete(route: &str) -> String {
    route
        .replace("{id}", "urn:ngsi-ld:Vehicle:matrix:1")
        .replace("{attr}", "speed")
        .replace("{type}", "Vehicle")
        .replace("{instance}", "urn:ngsi-ld:Instance:matrix:1")
}

/// What a route needs to reach its gate (`policy_gate_seam.rs`'s
/// `request_for`): the queries need a filter, the writes a body their
/// parser accepts.
fn carried(method: &str, route: &str) -> (&'static str, Value) {
    let entity = json!({"id": "urn:ngsi-ld:Vehicle:matrix:1", "type": "Vehicle",
                        "speed": {"type": "Property", "value": 1}});
    let fragment = json!({"speed": {"type": "Property", "value": 2}});
    let reads_as = if method == "HEAD" { "GET" } else { method };
    match (reads_as, route) {
        ("GET" | "DELETE", "/entities") | ("GET", "/entityMaps") => ("type=Vehicle", Value::Null),
        ("GET", "/temporal/entities" | "/temporal/entityMaps") => (
            "type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z",
            Value::Null,
        ),
        ("DELETE", "/snapshots") => ("q=status==%22completed%22", Value::Null),
        ("POST", "/entityMaps" | "/temporal/entityMaps" | "/entityOperations/query") => (
            "",
            json!({"type": "Query", "entities": [{"type": "Vehicle"}]}),
        ),
        ("POST", "/temporal/entityOperations/query") => (
            "",
            json!({"type": "Query", "entities": [{"type": "Vehicle"}],
                   "temporalQ": {"timerel": "after", "timeAt": "1970-01-01T00:00:00Z"}}),
        ),
        ("POST", p) if p.starts_with("/entityOperations") => ("", json!([entity])),
        ("POST", "/entities" | "/temporal/entities") => ("", entity),
        ("POST", "/subscriptions" | "/csourceSubscriptions") => (
            "",
            json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
                   "notification": {"endpoint": {"uri": "http://sink.invalid/n"}}}),
        ),
        ("POST", "/csourceRegistrations") => (
            "",
            json!({"type": "ContextSourceRegistration",
                   "information": [{"entities": [{"type": "Vehicle"}]}],
                   "endpoint": "http://csr.invalid/"}),
        ),
        ("POST", "/jsonldContexts") => (
            "",
            json!(["https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld"]),
        ),
        ("POST", "/snapshots") => ("", json!({"type": "Snapshot"})),
        ("POST", p) if p.ends_with("/clone") => ("", json!({})),
        ("POST" | "PATCH" | "PUT", _) => ("", fragment),
        _ => ("", Value::Null),
    }
}

/// One answer of `app`: the status and the body as text.
async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let Ok(res) = app.clone().call(req).await;
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Every route of the NGSI-LD router, each method the router's own OPTIONS
/// answer names, plus the `?details=true` variants of the two routes whose
/// operation a query parameter decides.
async fn cells() -> Vec<Cell> {
    let app = antares_api::router(AppState::new("matrix".into()));
    let mut out = Vec::new();
    for (route, _) in antares_api::api_routes() {
        let options = Request::builder()
            .method("OPTIONS")
            .uri(format!("{API}{}", concrete(route)))
            .body(Body::empty())
            .expect("request");
        let Ok(res) = app.clone().call(options).await;
        let allow = res
            .headers()
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(!allow.is_empty(), "OPTIONS {route} named no method");
        for method in allow.split(',').map(str::trim).filter(|m| *m != "OPTIONS") {
            let (query, body) = carried(method, route);
            out.push(Cell {
                method: method.to_owned(),
                route: route.to_owned(),
                query,
                body,
            });
        }
    }
    for route in ["/types", "/attributes"] {
        out.push(Cell {
            method: "GET".into(),
            route: route.into(),
            query: "details=true",
            body: Value::Null,
        });
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_route_of_every_surface_is_a_matrix_cell() {
    let text = std::fs::read_to_string(MATRIX).expect("the operation matrix");
    let listed = non_operation_routes(&text);
    assert!(!listed.is_empty(), "no non_operation_routes read");
    let cells = cells().await;
    // 69 is what the policy seam's walk finds, HEAD not counted
    assert!(cells.len() >= 69, "the route walk came out short");
    let mut reached = HashSet::new();
    let mut unknown = Vec::new();
    for c in &cells {
        match c.op() {
            Some(op) => {
                reached.insert(op.id());
            }
            None if listed
                .iter()
                .any(|(ms, p)| ms.contains(&c.method) && *p == c.path()) => {}
            None => unknown.push(format!("{} {}", c.method, c.route)),
        }
    }
    assert!(
        unknown.is_empty(),
        "routes that are no matrix cell: {unknown:?}"
    );
    // the broker-initiated rows (no gate clause) have no route
    let unreached: Vec<&str> = OperationId::ALL
        .iter()
        .filter(|op| op.gate_clause().is_some() && !reached.contains(op.id()))
        .map(|op| op.id())
        .collect();
    assert!(unreached.is_empty(), "rows no route serves: {unreached:?}");
}

/// Where a pass sends its requests.
#[derive(Clone, Copy, Debug)]
enum Pass {
    Root,
    View(&'static str),
}

/// The Tenant's `LinkML` schema: Vehicle with both Attributes, under the
/// core context's default vocabulary the views' names expand to.
const SCHEMA: &str = r#"{
  "id": "https://example.org/matrix", "name": "matrix",
  "prefixes": {"d": {"prefix_prefix": "d", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
  "default_prefix": "d",
  "classes": {"Vehicle": {"slots": ["speed", "secret", "owner"]}},
  "slots": {"speed": {"range": "integer"}, "secret": {}, "owner": {"range": "Vehicle"}}
}"#;

/// The OData surface, in a build that has it.
#[cfg(feature = "odata")]
fn odata() -> Option<Box<dyn antares_api::ApiSurface>> {
    Some(antares_ops::mount(antares_odata::OData))
}

#[cfg(not(feature = "odata"))]
fn odata() -> Option<Box<dyn antares_api::ApiSurface>> {
    None
}

/// The MCP surface, in a build that has it.
#[cfg(feature = "mcp")]
fn mcp() -> Option<Box<dyn antares_api::ApiSurface>> {
    Some(antares_ops::mount(antares_mcp::Mcp))
}

#[cfg(not(feature = "mcp"))]
fn mcp() -> Option<Box<dyn antares_api::ApiSurface>> {
    None
}

/// The WebSocket surfaces, Tenant root and view, in a build that has them.
#[cfg(feature = "ws")]
fn ws() -> Vec<Box<dyn antares_api::ApiSurface>> {
    vec![
        antares_ops::mount(antares_ws::Ws::process()),
        antares_ops::mount(antares_ws::ViewWs::process()),
    ]
}

#[cfg(not(feature = "ws"))]
fn ws() -> Vec<Box<dyn antares_api::ApiSurface>> {
    Vec::new()
}

/// The WebSocket notification sink, registered before `wire` and the
/// views share the sink registry (as `main.rs` does).
#[cfg(feature = "ws")]
fn ws_sink(state: AppState) -> AppState {
    state.with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())))
}

#[cfg(not(feature = "ws"))]
fn ws_sink(state: AppState) -> AppState {
    state
}

/// A `views` build's surfaces over the built-in store, asking `engine`,
/// with views `all` and `restricted` loaded, and Vehicle `matrix:1`
/// (speed and the hidden Attribute) in the views' Tenant.
async fn broker(engine: Clauses, writes: &HashSet<String>) -> (Router, antares_ops::views::Reload) {
    if cfg!(feature = "odata") {
        // The OData model is the Tenant's schema; a broker takes none
        // without a size limit.
        std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    }
    let mut state = ws_sink(AppState::new("matrix".into()));
    antares_api::wire(&mut state).await;
    let mut surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
    ];
    surfaces.extend(odata());
    surfaces.extend(mcp());
    surfaces.extend(ws());
    let state = state
        .with_policy(Arc::new(engine))
        .with_surfaces(surfaces)
        .expect("the surfaces mount");
    let reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    let app = antares_api::router(state);
    let all: Vec<&str> = OperationId::ALL.iter().map(|op| op.id()).collect();
    let reads: Vec<&str> = all
        .iter()
        .copied()
        .filter(|id| !writes.contains(*id))
        .collect();
    for (id, attributes, operations) in [
        ("all", vec!["speed", HIDDEN, "owner"], all),
        ("restricted", vec!["speed"], reads),
    ] {
        let doc = json!({"id": id, "tenant": TENANT, "selector": ["Vehicle"],
                         "rules": [{"types": ["Vehicle"], "attributes": attributes}],
                         "operations": operations});
        let put = Cell {
            method: "PUT".into(),
            route: String::new(),
            query: "",
            body: doc,
        };
        let mut req = put.request("");
        *req.uri_mut() = format!("/q/views/{id}").parse().expect("uri");
        let (status, body) = call(&app, req).await;
        assert!(status.is_success(), "view {id}: {status} {body}");
    }
    if cfg!(feature = "odata") {
        let schema = Cell {
            method: "PUT".into(),
            route: String::new(),
            query: "",
            body: serde_json::from_str(SCHEMA).expect("the schema"),
        };
        let mut req = schema.request("");
        *req.uri_mut() = format!("/q/schemas/{TENANT}").parse().expect("uri");
        let (status, body) = call(&app, req).await;
        assert!(status.is_success(), "schema: {status} {body}");
    }
    let vehicle = json!({"id": "urn:ngsi-ld:Vehicle:matrix:1", "type": "Vehicle",
                         "speed": {"type": "Property", "value": 1},
                         HIDDEN: {"type": "Property", "value": "s3"}});
    let seed = Cell {
        method: "POST".into(),
        route: "/entities".into(),
        query: "",
        body: vehicle,
    };
    let mut req = seed.request("");
    req.headers_mut()
        .insert("NGSILD-Tenant", TENANT.parse().expect("header"));
    let (status, body) = call(&app, req).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (app, reload)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_cell_is_reachable_or_rejected_as_stated() {
    let text = std::fs::read_to_string(MATRIX).expect("the operation matrix");
    let writes = writes(&text);
    assert!(writes.contains("create_entity") && !writes.contains("retrieve_entity"));
    let cells = cells().await;
    let mut wrong = Vec::new();
    for pass in [Pass::Root, Pass::View("all"), Pass::View("restricted")] {
        let engine = Clauses::default();
        let (app, _reload) = broker(engine.clone(), &writes).await;
        engine.take();
        let root = match pass {
            Pass::Root => String::new(),
            Pass::View(v) => format!("{}/{v}", antares_views::PREFIX),
        };
        for c in &cells {
            let Some(op) = c.op() else {
                continue;
            };
            let (status, body) = call(&app, c.request(&root)).await;
            let seen = engine.take();
            let served = match pass {
                Pass::Root => true,
                Pass::View(v) => {
                    antares_views::SERVED.contains(&op) && (v == "all" || !writes.contains(op.id()))
                }
            };
            let cell = format!("{pass:?} {} {}?{}", c.method, c.route, c.query);
            if served && seen != op.gate_clause().into_iter().collect::<Vec<_>>() {
                wrong.push(format!(
                    "{cell}: asked {seen:?}, the row's clause is {:?} ({status})",
                    op.gate_clause()
                ));
            }
            if !served && (!seen.is_empty() || ![404, 405, 422].contains(&status.as_u16())) {
                wrong.push(format!(
                    "{cell}: not served, answered {status} after {seen:?}"
                ));
            }
            if matches!(pass, Pass::View("restricted")) && body.contains(HIDDEN) {
                wrong.push(format!("{cell}: the hidden Attribute is in {body}"));
            }
        }
        // through a view, a path that is no cell is no route
        if let Pass::View(v) = pass {
            let nowhere = Request::get(format!("{root}{API}/nowhere"))
                .body(Body::empty())
                .expect("request");
            let (status, _) = call(&app, nowhere).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
            assert!(
                engine.take().is_empty(),
                "{v}: a non-route asked the engine"
            );
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The `odata:` status of every row of the operation matrix.
#[cfg(feature = "odata")]
fn odata_status(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut row = None;
    let mut inside = false;
    for l in section(text, "operations") {
        if let Some(id) = l.strip_prefix("- id: ") {
            row = Some(id.trim().to_owned());
            inside = false;
        } else if l == "    odata:" {
            inside = true;
        } else if let Some(status) = l.strip_prefix("      status: ").filter(|_| inside) {
            out.extend(row.take().map(|id| (id, status.trim().to_owned())));
            inside = false;
        }
    }
    out
}

/// The OData column (T-3508): every row the matrix marks `odata:
/// supported` is sent in its OData form through views `all` and
/// `restricted`. A read this round serves asks the engine once, with the
/// row's clause; a document the model answers asks nothing; a write is
/// T-3509's and is refused before the engine. A row marked supported that
/// this column does not send is red.
#[cfg(feature = "odata")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_odata_column_is_served_as_the_matrix_states() {
    const KEY: &str = "Vehicle('urn:ngsi-ld:Vehicle:matrix:1')";
    /// How a row answers.
    enum Want {
        /// 200 after asking the engine once, with the row's clause.
        Gated,
        /// A write with this body: under view `all` a 2xx after asking
        /// the engine once, with the row's clause; under `restricted`,
        /// which offers no write, 405 before the engine.
        Write(&'static str),
        /// A `$ref` write with this body: under `all` the target read
        /// (5.7.1) and then the row's clause; under `restricted`, whose
        /// model has no navigation, 404 before the engine.
        Linked(&'static str),
        /// A `PUT` of the Entity: under `all` the Entity read (5.7.1, its named instances
        /// checked, T-5206) and then the row's clause; under `restricted` 405 before the engine.
        Replaced(&'static str),
    }
    let sent: &[(&str, &str, String, Want)] = &[
        (
            "query_entities",
            "GET",
            "Vehicle?$top=1".into(),
            Want::Gated,
        ),
        ("retrieve_entity", "GET", KEY.into(), Want::Gated),
        (
            "retrieve_attribute_value",
            "GET",
            format!("{KEY}/speed"),
            Want::Gated,
        ),
        // the documents naming the model ask the engine (F-ANT4-003)
        ("retrieve_entity_types", "GET", String::new(), Want::Gated),
        (
            "retrieve_entity_type_details",
            "GET",
            "$metadata".into(),
            Want::Gated,
        ),
        // paging is by offset: no EntityMap is made (the row's binding says "may")
        (
            "create_entity_map",
            "GET",
            "Vehicle?$top=1&$skip=1".into(),
            Want::Gated,
        ),
        (
            "create_entity",
            "POST",
            "Vehicle".into(),
            Want::Write(r#"{"id": "urn:ngsi-ld:Vehicle:matrix:2", "speed": 2}"#),
        ),
        (
            "merge_entity",
            "PATCH",
            KEY.into(),
            Want::Write(r#"{"speed": 3}"#),
        ),
        (
            "replace_entity",
            "PUT",
            KEY.into(),
            Want::Replaced(r#"{"speed": 4}"#),
        ),
        (
            "delete_entity",
            "DELETE",
            "Vehicle('urn:ngsi-ld:Vehicle:matrix:2')".into(),
            Want::Write(""),
        ),
        (
            "append_attributes",
            "PUT",
            format!("{KEY}/owner/$ref"),
            Want::Linked(r#"{"@odata.id": "Vehicle('urn:ngsi-ld:Vehicle:matrix:1')"}"#),
        ),
        (
            "partial_attribute_update",
            "PATCH",
            format!("{KEY}/speed"),
            Want::Write(r#"{"value": 5}"#),
        ),
        (
            "replace_attribute",
            "PUT",
            format!("{KEY}/speed"),
            Want::Write(r#"{"value": 6}"#),
        ),
        (
            "delete_attribute",
            "DELETE",
            format!("{KEY}/speed"),
            Want::Write(""),
        ),
    ];
    let text = std::fs::read_to_string(MATRIX).expect("the operation matrix");
    let supported: Vec<String> = odata_status(&text)
        .into_iter()
        .filter(|(_, status)| status == "supported")
        .map(|(id, _)| id)
        .collect();
    assert!(supported.len() >= 10, "the column is read: {supported:?}");
    let mut wrong: Vec<String> = supported
        .iter()
        .filter(|id| !sent.iter().any(|(row, ..)| row == id))
        .map(|id| format!("{id}: marked supported, not sent"))
        .collect();
    let writes = writes(&text);
    for view in ["all", "restricted"] {
        let engine = Clauses::default();
        let (app, _reload) = broker(engine.clone(), &writes).await;
        engine.take();
        for (row, method, path, want) in sent {
            if !supported.iter().any(|id| id == row) {
                wrong.push(format!(
                    "{row}: sent, but the matrix does not mark it supported"
                ));
                continue;
            }
            let body = match want {
                Want::Write(body) | Want::Linked(body) | Want::Replaced(body) => body,
                _ if *method == "GET" || *method == "DELETE" => "",
                _ => "{}",
            };
            // A create answers without reading the Entity back: one ask.
            let mut req = Request::builder()
                .method(*method)
                .uri(format!("/x/views/{view}/odata/v4/{path}"))
                .header("Prefer", "return=minimal");
            if !body.is_empty() {
                req = req
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len());
            }
            let req = req.body(Body::from(body)).expect("request");
            let (status, answer) = call(&app, req).await;
            let seen = engine.take();
            let cell = format!("{view} {row}: {method} {path}");
            let op = OperationId::from_id(row);
            let clause: Vec<&str> = op.and_then(OperationId::gate_clause).into_iter().collect();
            let read = !writes.contains(*row);
            match want {
                Want::Gated if status != StatusCode::OK => {
                    wrong.push(format!("{cell}: {status} {answer}"));
                }
                Want::Gated if *row == "create_entity_map" => {
                    if seen != ["5.7.2"] {
                        wrong.push(format!(
                            "{cell}: asked {seen:?}, paging is one Query Entities"
                        ));
                    }
                }
                Want::Gated if seen != clause => {
                    wrong.push(format!(
                        "{cell}: asked {seen:?}, the row's clause is {clause:?}"
                    ));
                }
                Want::Write(_) if view == "all" && (!status.is_success() || seen != clause) => {
                    wrong.push(format!(
                        "{cell}: {status} {answer} after {seen:?}, the row's clause is {clause:?}"
                    ));
                }
                Want::Linked(_) | Want::Replaced(_)
                    if view == "all"
                        && (!status.is_success()
                            || seen.first() != Some(&"5.7.1")
                            || seen.get(1..) != Some(&clause[..])) =>
                {
                    wrong.push(format!(
                        "{cell}: {status} {answer} after {seen:?}, the target read and then {clause:?}"
                    ));
                }
                Want::Linked(_)
                    if view == "restricted"
                        && (status != StatusCode::NOT_FOUND || !seen.is_empty()) =>
                {
                    wrong.push(format!("{cell}: answered {status} after {seen:?}"));
                }
                Want::Write(_) | Want::Replaced(_)
                    if view == "restricted"
                        && (status != StatusCode::METHOD_NOT_ALLOWED || !seen.is_empty()) =>
                {
                    wrong.push(format!(
                        "{cell}: a write the view does not offer answered {status} after {seen:?}"
                    ));
                }
                _ => {}
            }
            if view == "restricted" && answer.contains(HIDDEN) {
                wrong.push(format!("{cell}: the hidden Attribute is in {answer}"));
            }
            assert!(
                read != matches!(want, Want::Write(_) | Want::Linked(_) | Want::Replaced(_)),
                "{cell}: a read sent as a write or a write as a read"
            );
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The `mcp:` status and binding of every row: `(id, status, binding)`.
#[cfg(feature = "mcp")]
fn mcp_cells(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut row = String::new();
    let mut status = None;
    let mut inside = false;
    for l in section(text, "operations") {
        if let Some(id) = l.strip_prefix("- id: ") {
            row = id.trim().to_owned();
            inside = false;
        } else if l == "    mcp:" {
            inside = true;
        } else if let Some(s) = l.strip_prefix("      status: ").filter(|_| inside) {
            status = Some(s.trim().to_owned());
        } else if let Some(b) = l.strip_prefix("      binding: ").filter(|_| inside) {
            out.extend(status.take().map(|s| (row.clone(), s, b.trim().to_owned())));
            inside = false;
        }
    }
    out
}

/// The MCP column (T-3656).
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mcp_column_is_served_as_the_matrix_states() {
    const ID: &str = "urn:ngsi-ld:Vehicle:matrix:1";
    const HOOKS: &str = "https://hooks.example/";
    let time = |more: Value| {
        let mut args = json!({"timerel": "before", "timeAt": "2999-01-01T00:00:00Z"});
        if let (Some(a), Value::Object(more)) = (args.as_object_mut(), more) {
            a.extend(more);
        }
        args
    };
    let vehicle = |n: &str, speed: i64| {
        json!({"id": format!("urn:ngsi-ld:Vehicle:{n}"), "type": "Vehicle",
               "speed": {"type": "Property", "value": speed}})
    };
    let prop = |v: Value| json!({"type": "Property", "value": v});
    let m2 = "urn:ngsi-ld:Vehicle:m2";
    // every tool, in an order whose writes find what they change; `{sub}`
    // is the id create_subscription made
    let sent: Vec<(&str, Value)> = vec![
        ("get_entity", json!({"id": ID})),
        ("query_entities", json!({"type": ["Vehicle"]})),
        ("batch_query", json!({"type": ["Vehicle"]})),
        ("get_attribute_value", json!({"id": ID, "attrId": "speed"})),
        ("list_types", json!({})),
        ("list_type_details", json!({})),
        ("get_type", json!({"type": "Vehicle"})),
        ("list_attributes", json!({})),
        ("list_attribute_details", json!({})),
        ("get_attribute", json!({"attrId": "speed"})),
        ("retrieve_temporal", time(json!({"id": ID}))),
        ("query_temporal", time(json!({"type": ["Vehicle"]}))),
        ("batch_query_temporal", time(json!({"type": ["Vehicle"]}))),
        ("create_entity", json!({"entity": vehicle("m2", 5)})),
        (
            "update_attributes",
            json!({"id": m2, "attributes": {"speed": prop(json!(6))}}),
        ),
        (
            "append_attributes",
            json!({"id": m2, "attributes": {HIDDEN: prop(json!("x"))}, "noOverwrite": true}),
        ),
        (
            "update_attribute",
            json!({"id": m2, "attrId": "speed", "attribute": {"value": 7}}),
        ),
        (
            "replace_attribute",
            json!({"id": m2, "attrId": "speed", "attribute": prop(json!(8))}),
        ),
        (
            "merge_entity",
            json!({"id": m2, "fragment": {HIDDEN: prop(json!("y"))}}),
        ),
        ("delete_attribute", json!({"id": m2, "attrId": HIDDEN})),
        (
            "replace_entity",
            json!({"id": m2, "entity": vehicle("m2", 9)}),
        ),
        ("batch_create", json!({"entities": [vehicle("m3", 1)]})),
        (
            "batch_update",
            json!({"entities": [vehicle("m3", 3)], "noOverwrite": false}),
        ),
        ("batch_merge", json!({"entities": [vehicle("m3", 4)]})),
        (
            "batch_upsert",
            json!({"entities": [vehicle("m4", 2)], "options": "update"}),
        ),
        ("batch_delete", json!({"ids": ["urn:ngsi-ld:Vehicle:m3"]})),
        ("delete_entity", json!({"id": m2})),
        (
            "create_subscription",
            json!({"subscription": {"type": "Subscription", "entities": [{"type": "Vehicle"}],
                                    "notification": {"endpoint": {"uri": format!("{HOOKS}a")}}}}),
        ),
        ("get_subscription", json!({"id": "{sub}"})),
        ("list_subscriptions", json!({})),
        (
            "update_subscription",
            json!({"id": "{sub}", "fragment": {"description": "matrix"}}),
        ),
        ("delete_subscription", json!({"id": "{sub}"})),
    ];
    // ponytail: the allowlist is read once per process
    std::env::set_var(antares_mcp::SUBSCRIPTION_ENDPOINTS_ENV, HOOKS);
    let text = std::fs::read_to_string(MATRIX).expect("the operation matrix");
    let writes = writes(&text);
    let cells = mcp_cells(&text);
    assert!(cells.len() >= 60, "the column is read: {}", cells.len());
    let mut wrong: Vec<String> = Vec::new();
    // tool -> row, from the matrix; a supported cell names one tool
    let mut rows = std::collections::BTreeMap::new();
    for (row, status, binding) in &cells {
        match status.as_str() {
            "supported" if binding.contains(' ') || binding == "null" => {
                wrong.push(format!(
                    "{row}: supported, but names no single tool: {binding}"
                ));
            }
            "supported" => {
                if let Some(other) = rows.insert(binding.clone(), row.clone()) {
                    wrong.push(format!("{binding}: the tool of {other} and of {row}"));
                }
            }
            "not_offered" | "rejected" => {}
            other => wrong.push(format!("{row}: mcp status {other}")),
        }
    }
    let named: Vec<&str> = sent.iter().map(|(tool, _)| *tool).collect();
    for tool in rows.keys() {
        if !named.contains(&tool.as_str()) {
            wrong.push(format!("{tool}: in the matrix, not called here"));
        }
    }
    for view in ["all", "restricted"] {
        let engine = Clauses::default();
        let (app, _reload) = broker(engine.clone(), &writes).await;
        let mut listed = mcp_tools(&app, view).await;
        // `describe_type` is `get_type`'s row answered with its guide (T-4537), not a row
        let guide = listed.iter().position(|t| t == "describe_type");
        assert_eq!(
            guide.is_some(),
            listed.iter().any(|t| t == "get_type"),
            "{view}: describe_type is listed exactly where get_type is: {listed:?}"
        );
        // `summarize` is `query_entities`' row answered as a profile (T-4680), not a row
        assert_eq!(
            listed.iter().any(|t| t == "summarize"),
            listed.iter().any(|t| t == "query_entities"),
            "{view}: summarize is listed exactly where query_entities is: {listed:?}"
        );
        // `area_overview` is the same row answered per type for one area (T-5279)
        assert_eq!(
            listed.iter().any(|t| t == "area_overview"),
            listed.iter().any(|t| t == "query_entities"),
            "{view}: area_overview is listed exactly where query_entities is: {listed:?}"
        );
        listed.retain(|t| t != "describe_type" && t != "summarize" && t != "area_overview");
        listed.sort();
        let mut want: Vec<String> = rows
            .iter()
            .filter(|(_, row)| view == "all" || !writes.contains(row.as_str()))
            .map(|(tool, _)| tool.clone())
            .collect();
        want.sort();
        if listed != want {
            wrong.push(format!(
                "{view}: tools/list {listed:?}, the matrix names {want:?}"
            ));
        }
        engine.take();
        // `restricted` makes no Subscription: it reads one that is not there
        let mut sub = String::from("urn:ngsi-ld:Subscription:none");
        for (tool, args) in &sent {
            let Some(row) = rows.get(*tool) else {
                wrong.push(format!("{tool}: called here, no matrix row names it"));
                continue;
            };
            let clause: Vec<&str> = OperationId::from_id(row)
                .and_then(OperationId::gate_clause)
                .into_iter()
                .collect();
            let args: Value =
                serde_json::from_str(&args.to_string().replace("{sub}", &sub)).expect("json");
            let cell = format!("{view} {tool} ({row})");
            let write = writes.contains(row.as_str());
            if view == "restricted" && write {
                let a = mcp_call(&app, view, tool, &args, None).await;
                let seen = engine.take();
                if a.get("error").is_none() || !seen.is_empty() {
                    wrong.push(format!(
                        "{cell}: a tool the view hides answered {a} after {seen:?}"
                    ));
                }
                continue;
            }
            let mut a = mcp_call(&app, view, tool, &args, None).await;
            if write {
                let asked = engine.take();
                let state = a
                    .pointer("/result/requestState")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let Some(state) = state else {
                    wrong.push(format!("{cell}: not asked: {a}"));
                    continue;
                };
                if asked != clause {
                    wrong.push(format!(
                        "{cell}: asking the person asked {asked:?}, the probe is {clause:?}"
                    ));
                }
                a = mcp_call(&app, view, tool, &args, Some(&state)).await;
            }
            let seen = engine.take();
            let probe_and_gate: Vec<&str> = clause.iter().chain(&clause).copied().collect();
            let absent = view == "restricted" && *tool == "get_subscription";
            let failed = a.pointer("/result/isError") == Some(&Value::Bool(true))
                || a.get("error").is_some();
            let status = a.pointer("/result/structuredContent/error/status");
            if failed != absent || (absent && status != Some(&json!(404))) {
                wrong.push(format!("{cell}: {a}"));
            } else if seen != probe_and_gate {
                wrong.push(format!(
                    "{cell}: asked {seen:?}, one probe and the row's clause is {probe_and_gate:?}"
                ));
            }
            if view == "restricted" && a.to_string().contains(HIDDEN) {
                wrong.push(format!("{cell}: the hidden Attribute is in {a}"));
            }
            if *tool == "create_subscription" {
                sub = a
                    .pointer("/result/structuredContent/created")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// One 2026-07-28 JSON-RPC request to view `view`'s MCP endpoint by a
/// client that can ask the person; the answer's JSON.
#[cfg(feature = "mcp")]
async fn mcp_rpc(app: &Router, view: &str, method: &str, mut params: Value) -> Value {
    const MODERN: &str = "2026-07-28";
    if let Some(p) = params.as_object_mut() {
        p.insert(
            "_meta".into(),
            json!({
                "io.modelcontextprotocol/protocolVersion": MODERN,
                "io.modelcontextprotocol/clientInfo": {"name": "matrix", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {"elicitation": {"form": {}}}
            }),
        );
    }
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let mut req = Request::post(format!("{}/{view}/mcp", antares_views::PREFIX))
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", MODERN)
        .header("Mcp-Method", method)
        .header("Content-Length", body.len());
    if let Some(n) = name {
        req = req.header("Mcp-Name", n);
    }
    let (status, text) = call(app, req.body(Body::from(body)).expect("request")).await;
    serde_json::from_str(&text).unwrap_or_else(|_| json!({"status": status.as_u16(), "text": text}))
}

/// The names `tools/list` gives on `view`.
#[cfg(feature = "mcp")]
async fn mcp_tools(app: &Router, view: &str) -> Vec<String> {
    let a = mcp_rpc(app, view, "tools/list", json!({})).await;
    a.pointer("/result/tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// `tools/call` of `tool`; with `state`, the retry the person confirmed.
#[cfg(feature = "mcp")]
async fn mcp_call(
    app: &Router,
    view: &str,
    tool: &str,
    args: &Value,
    state: Option<&str>,
) -> Value {
    let mut params = json!({"name": tool, "arguments": args});
    if let (Some(p), Some(state)) = (params.as_object_mut(), state) {
        p.insert("requestState".into(), json!(state));
        p.insert(
            "inputResponses".into(),
            json!({"confirm": {"action": "accept", "content": {"confirm": true}}}),
        );
    }
    mcp_rpc(app, view, "tools/call", params).await
}

/// The `websocket:` status and binding of every row: `(id, status, binding)`.
#[cfg(feature = "ws")]
fn ws_cells(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut row = String::new();
    let mut status = None;
    let mut inside = false;
    let mut binding: Option<String> = None;
    let mut flush = |row: &str, status: &mut Option<String>, binding: &mut Option<String>| {
        if let Some(b) = binding.take() {
            let s = status.take().unwrap_or_default();
            out.push((row.to_owned(), s, b.trim().trim_matches('\'').to_owned()));
        }
    };
    for l in section(text, "operations") {
        if let Some(b) = binding.as_mut().filter(|_| l.starts_with("        ")) {
            // a folded binding continues on the deeper lines
            b.push(' ');
            b.push_str(l.trim());
            continue;
        }
        flush(&row, &mut status, &mut binding);
        if let Some(id) = l.strip_prefix("- id: ") {
            row = id.trim().to_owned();
            inside = false;
        } else if l == "    websocket:" {
            inside = true;
        } else if let Some(st) = l.strip_prefix("      status: ").filter(|_| inside) {
            status = Some(st.trim().to_owned());
        } else if let Some(b) = l.strip_prefix("      binding: ").filter(|_| inside) {
            binding = Some(b.to_owned());
            inside = false;
        }
    }
    flush(&row, &mut status, &mut binding);
    out
}

/// The `x-op` names under `surfaces.websocket.session_messages`.
#[cfg(feature = "ws")]
fn session_messages(text: &str) -> Vec<String> {
    section(text, "surfaces")
        .skip_while(|l| *l != "  websocket:")
        .skip_while(|l| *l != "    session_messages:")
        .skip(1)
        .take_while(|l| l.starts_with("      "))
        .filter_map(|l| l.strip_prefix("      ")?.split_once(':'))
        .filter(|(k, _)| !k.starts_with(' '))
        .map(|(k, _)| k.to_owned())
        .collect()
}

/// The `x-op` names the `antares-ws` envelope reads: the renames of its
/// `OpName`, from the source, so an op added there without a row is red.
#[cfg(feature = "ws")]
fn served_ops() -> std::collections::BTreeSet<String> {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../antares-ws/src/envelope.rs"
    ))
    .expect("the envelope source");
    let start = src.find("enum OpName {").expect("enum OpName");
    let body = &src[start..];
    let body = &body[..body.find("\n}").expect("the end of OpName")];
    body.split("rename = \"")
        .skip(1)
        .filter_map(|r| r.split_once('"').map(|(name, _)| name.to_owned()))
        .collect()
}

/// The WebSocket column (T-3696).
#[cfg(feature = "ws")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_websocket_column_is_served_as_the_matrix_states() {
    use std::collections::BTreeSet;
    #[path = "common/ws_client.rs"]
    mod ws_client;
    use ws_client::Socket;

    let text = std::fs::read_to_string(MATRIX).expect("the operation matrix");
    let cells = ws_cells(&text);
    assert_eq!(
        cells.len(),
        OperationId::ALL.len(),
        "a row without a websocket cell"
    );
    let supported: Vec<(String, String)> = cells
        .into_iter()
        .filter(|(_, status, _)| status == "supported")
        .map(|(row, _, binding)| (row, binding))
        .collect();
    let session = session_messages(&text);
    assert_eq!(session, ["x-resume"], "session_messages read");
    let op_of = |binding: &str| binding.split(':').next().unwrap_or_default().to_owned();
    let stated: BTreeSet<String> = supported
        .iter()
        .map(|(_, b)| op_of(b))
        .filter(|op| op.starts_with("x-"))
        .chain(session.iter().cloned())
        .collect();
    assert_eq!(
        stated,
        served_ops(),
        "the envelope's x-ops and the matrix's websocket bindings differ"
    );
    let clause = |row: &str| {
        OperationId::ALL
            .iter()
            .find(|op| op.id() == row)
            .and_then(|op| op.gate_clause())
    };
    let retrieve = clause("retrieve_subscription").expect("5.8.3");
    // each supported row: (x-op, the clauses it asks, in order)
    let mut asks: Vec<(String, Vec<&'static str>)> = Vec::new();
    let mut pushed = false;
    for (row, binding) in &supported {
        let op = op_of(binding);
        if !op.starts_with("x-") {
            assert!(
                op == "server push" && clause(row).is_none(),
                "{row}: a websocket binding is an x-op or the server push of a broker-initiated row"
            );
            pushed = true;
            continue;
        }
        let mut c = vec![clause(row).unwrap_or_else(|| panic!("{row} has no gate clause"))];
        if binding.contains("then Retrieve Subscription") {
            c.push(retrieve);
        }
        // ADR-0026 rule 4 (T-4755): the socket's own Subscription is
        // decided as Query Entities, whatever its row's own clause
        let c = c
            .into_iter()
            .map(|c| if c.starts_with("5.8.") { "5.7.2" } else { c })
            .collect();
        asks.push((op, c));
    }
    assert!(pushed, "no row states the notification push");
    let patch_clause = clause("update_attributes").expect("5.6.2");

    let engine = Clauses::default();
    let (app, _reload) = broker(engine.clone(), &writes(&text)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    let rt = tokio::runtime::Handle::current();

    let failures = tokio::task::spawn_blocking(move || {
        let mut wrong = Vec::new();
        let message = |op: &str, sub: &str| match op {
            "x-subscribe" => json!({"metadata": {"x-op": op, "x-id": op},
                "body": {"type": "Subscription", "entities": [{"type": "Vehicle"}]}}),
            "x-update" => json!({"metadata": {"x-op": op, "x-id": op, "x-subscription": sub},
                "body": {"description": "matrix"}}),
            "x-unsubscribe" => json!({"metadata": {"x-op": op, "x-id": op, "x-subscription": sub}}),
            other => {
                panic!("no message for {other}: the column sends every x-op the matrix states")
            }
        };
        let open = |path: &str, tenant: Option<&str>| {
            let mut ws = Socket::open(
                port,
                path,
                &tenant
                    .map(|t| ("NGSILD-Tenant".to_owned(), t.to_owned()))
                    .into_iter()
                    .collect::<Vec<_>>(),
                std::time::Duration::from_secs(2),
            )
            .expect("the upgrade");
            let welcome = ws.next().expect("the welcome");
            assert_eq!(welcome.pointer("/metadata/x-op"), Some(&json!("x-welcome")));
            engine.take();
            ws
        };
        let status = |m: &Option<Value>| {
            m.as_ref()
                .and_then(|m| m.pointer("/metadata/x-status").cloned())
        };
        for (speed, path, tenant) in [
            (2, "/x/ws/ngsi-ld/v1", Some(TENANT)),
            (3, "/x/views/all/ws", None),
        ] {
            let mut ws = open(path, tenant);
            let mut sub = String::from("urn:ngsi-ld:Subscription:none");
            for (op, clauses) in &asks {
                ws.send(&message(op, &sub)).expect("send");
                let answer = ws.next();
                let st = status(&answer).and_then(|s| s.as_u64()).unwrap_or(0);
                if !(200..300).contains(&st) {
                    wrong.push(format!("{path} {op}: {answer:?}"));
                }
                if op == "x-subscribe" {
                    sub = answer
                        .as_ref()
                        .and_then(|a| a.pointer("/body/id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    // a change is pushed and asks nothing beyond its own gate
                    let patch_body =
                        json!({"speed": {"type": "Property", "value": speed}}).to_string();
                    let patch = Request::builder()
                        .method("PATCH")
                        .uri(format!("{API}/entities/urn:ngsi-ld:Vehicle:matrix:1/attrs"))
                        .header("NGSILD-Tenant", TENANT)
                        .header("Content-Type", "application/json")
                        .header("Content-Length", patch_body.len())
                        .body(Body::from(patch_body))
                        .expect("request");
                    let (code, body) = rt.block_on(call(&app, patch));
                    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
                    let got = std::iter::from_fn(|| ws.next())
                        .find(|m| m.pointer("/metadata/x-op") == Some(&json!("x-notification")));
                    if got.is_none() {
                        wrong.push(format!("{path}: no notification of the change"));
                    }
                    let seen = engine.take();
                    if seen != [clauses.as_slice(), &[patch_clause]].concat() {
                        wrong.push(format!("{path} {op} and the push asked {seen:?}"));
                    }
                    continue;
                }
                let seen = engine.take();
                if seen != *clauses {
                    wrong.push(format!(
                        "{path} {op} asked {seen:?}, the matrix states {clauses:?}"
                    ));
                }
            }
        }
        // refused before the engine: the writes through a read-only view,
        // an x-op no row names, and x-resume of no connection
        let mut ws = open("/x/views/restricted/ws", None);
        for (op, _) in &asks {
            ws.send(&message(op, "urn:ngsi-ld:Subscription:none"))
                .expect("send");
            let answer = ws.next();
            let st = status(&answer).and_then(|s| s.as_u64()).unwrap_or(0);
            let seen = engine.take();
            if !(400..500).contains(&st) || !seen.is_empty() {
                wrong.push(format!("restricted {op}: {answer:?}, asked {seen:?}"));
            }
        }
        ws.send(&json!({"metadata": {"x-op": "x-query-entities", "x-id": "q"}}))
            .expect("send");
        let answer = ws.next();
        if status(&answer) != Some(json!(400)) || !engine.take().is_empty() {
            wrong.push(format!("an unknown x-op: {answer:?}"));
        }
        let mut ws = open("/x/ws/ngsi-ld/v1", Some(TENANT));
        ws.send(&json!({"metadata": {"x-op": "x-resume", "x-id": "r",
            "x-connection": "none", "x-resume-token": "none", "x-seq": 0}}))
            .expect("send");
        let answer = ws.next();
        if status(&answer) != Some(json!(404)) || !engine.take().is_empty() {
            wrong.push(format!("x-resume: {answer:?}"));
        }
        wrong
    })
    .await
    .expect("the socket client");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
