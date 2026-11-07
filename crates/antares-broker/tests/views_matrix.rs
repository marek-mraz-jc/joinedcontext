// SPDX-License-Identifier: EUPL-1.2
//! The view and policy matrix (T-3777): every scenario of
//! `tests/views-matrix/` (a view, the policies of the joinedcontext engine,
//! a caller, the Entities and Attributes NGSI-LD admits, written by hand
//! from the seed) is read through every surface of a `full,jc-policy`
//! build, and each must answer exactly the expected Entities with exactly
//! the expected Attribute names: no more, no fewer.
//!
//! Each scenario runs in a Tenant of its own over the same seed
//! (`_seed.json`) and LinkML schema (`_schema.json`). A surface is read
//! page by page, so a next link or cursor that leaked or lost an Entity is
//! red, and its count (`NGSILD-Results-Count`, `@odata.count`, MCP
//! `total`, OGC `numberMatched`) must be the expected size. A retrieve of a
//! seed Entity the scenario does not admit must answer the bytes an absent
//! id gets. A denied caller gets a refusal naming no seed Entity.
//!
//! With `internal-csr`, each scenario is read once more through a
//! registration of this broker in a second Tenant (ADR-0025): the forward
//! must answer what the view admits, and nothing to a denied caller.
//!
//! A scenario may carry `steps`: a move of an Entity (written on the
//! Tenant root, outside the engine) or a write by the caller through the
//! view, each with the expected answer after it.
#![cfg(all(feature = "full", feature = "jc-policy"))]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use proptest::prelude::*;
use proptest::test_runner::{RngSeed, TestRunner};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tower::Service;

#[path = "common/ws_client.rs"]
mod ws_client;
use ws_client::Socket;

const SEED: &str = include_str!("views-matrix/_seed.json");
const SCHEMA: &str = include_str!("views-matrix/_schema.json");
/// The types of the seed, in the order a `*` view is queried by.
const TYPES: [&str; 3] = ["Vehicle", "Parking", "Sensor"];
/// The page size every surface is read with: small, so every answer pages.
const PAGE: usize = 3;
/// More pages than any answer of the seed needs: a next link past it never ends.
const MAX_PAGES: usize = 50;
/// The read and subscription clauses a scenario without policies grants
/// the public role whole, so the view alone decides.
const READS: [&str; 15] = [
    "5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.7.5", "5.7.6", "5.7.7", "5.7.8", "5.7.9", "5.7.10",
    "5.8.1", "5.8.2", "5.8.3", "5.8.4", "5.8.5",
];
/// The service account the harness writes the seed and the moves as: a
/// permission of its own in every document, never the scenario's caller.
const SEEDER: &str = "matrix-seed";
/// The mesh identity the broker believes a subject from.
const GATEWAY: &str = "context-gateway.jc.serviceaccount.identity.linkerd.cluster.local";

/// The members of an Entity that are not Attributes.
const FRAME: [&str; 7] = [
    "id",
    "type",
    "scope",
    "@context",
    "createdAt",
    "modifiedAt",
    "deletedAt",
];

/// Entity id → its Attribute names.
type Answer = BTreeMap<String, BTreeSet<String>>;

/// What a scenario expects: the Entities and their Attributes, or a refusal.
#[derive(Debug, Clone, PartialEq)]
enum Expect {
    Admitted(Answer),
    Denied,
}

fn expect(v: &Value) -> Expect {
    if v == "denied" {
        return Expect::Denied;
    }
    Expect::Admitted(
        v.as_object()
            .into_iter()
            .flatten()
            .map(|(id, names)| {
                let names = names
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                (id.clone(), names)
            })
            .collect(),
    )
}

struct Scenario {
    name: String,
    tenant: String,
    doc: Value,
}

impl Scenario {
    fn view(&self) -> &str {
        &self.tenant
    }

    /// The types a read asks for: the view's selector, or every seed type
    /// for a `*` view.
    fn types(&self) -> Vec<String> {
        let selector: Vec<String> = self.doc["view"]["selector"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        if selector.iter().any(|t| t == "*") {
            TYPES.map(str::to_owned).to_vec()
        } else {
            selector
        }
    }

    fn caller(&self) -> Vec<(String, String)> {
        self.doc["caller"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
            .collect()
    }

    /// The rule document: the scenario's policies (the public role reading
    /// everything when it has none) and its view binding.
    fn policy(&self) -> Value {
        let policies = match self.doc.get("policies") {
            Some(p) => p.clone(),
            None => json!([{"id": "open", "assignee": {"kind": "role", "id": "public"},
                            "operations": READS}]),
        };
        let mut doc = json!({ "policies": policies });
        if let Some(all) = doc["policies"].as_array_mut() {
            all.push(
                json!({"id": "matrix-seed", "assignee": {"kind": "serviceAccount", "id": SEEDER},
                            "operations": ["5.6.1", "5.6.2", "5.6.8", "5.6.10"],
                            "information": [{"entities": TYPES.map(|t| json!({"type": t}))}]}),
            );
        }
        if let Some(bound) = self.doc.get("bind") {
            doc["views"] = json!({ self.view(): bound });
        }
        doc
    }
}

fn corpus() -> Vec<Scenario> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/views-matrix");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('_'))
        })
        .collect();
    files.sort();
    files
        .iter()
        .enumerate()
        .map(|(n, path)| {
            let text = std::fs::read_to_string(path).expect("a scenario");
            Scenario {
                name: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_owned(),
                tenant: format!("m{n:02}"),
                doc: serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
            }
        })
        .collect()
}

/// The broker of a `full` build with the joinedcontext engine set before
/// it is wired, as `main` sets it, so notifications are decided by it too.
async fn broker() -> (Router, antares_ops::views::Reload) {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    std::env::set_var(
        "ANTARES_POLICY_SUBJECT_HEADERS",
        joinedcontext_policy::subject::HEADERS.join(","),
    );
    let mut state = AppState::new("views-matrix".into())
        .with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())))
        .with_policy(std::sync::Arc::new(
            joinedcontext_policy::JoinedcontextPolicy::new(),
        ));
    // As a deployment behind the gateway runs (F-ANT2-001): a subject is
    // believed only from the gateway's mesh identity, which every request
    // of this matrix carries, so each surface's in-process calls (views,
    // MCP, the internal Context Source) run under that wall too.
    state.subject_required = true;
    state.provenance_from = Some(GATEWAY.to_owned());
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        antares_ops::mount(antares_odata::OData),
        antares_ops::mount(antares_ogc::Ogc),
        antares_ops::mount(antares_mcp::Mcp),
        antares_ops::mount(antares_ws::ViewWs::process()),
    ];
    #[cfg(feature = "linkml")]
    let surfaces = {
        let mut all = surfaces;
        all.push(antares_ops::mount(antares_linkml::Linkml));
        all
    };
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    let reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    #[cfg(feature = "internal-csr")]
    let state = antares_ops::with_forward_transport(
        state,
        antares_icsr::transport(antares_icsr::SelfUrls::parse(SELF_URL).expect("a self URL")),
    )
    .expect("the internal transport");
    (antares_api::router(state), reload)
}

/// One request; status, headers and the JSON body (the text as a string
/// when it is not JSON).
async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Option<&Value>,
) -> (StatusCode, HeaderMap, Value) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost")
        .header("l5d-client-id", GATEWAY);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if body.is_some() {
        let kind = if method == "PATCH" && uri.contains("/ogc/") {
            "application/merge-patch+json"
        } else {
            "application/json"
        };
        req = req
            .header("Content-Type", kind)
            .header("Content-Length", text.len());
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("a request"))
        .await;
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, headers, body)
}

fn tenant_header(s: &Scenario) -> (String, String) {
    ("NGSILD-Tenant".to_owned(), s.tenant.clone())
}

fn seeder(s: &Scenario) -> Vec<(String, String)> {
    vec![
        ("x-jc-client".to_owned(), SEEDER.to_owned()),
        tenant_header(s),
    ]
}

/// The view, schema and rule document of `s`, in its Tenant.
async fn install(app: &Router, s: &Scenario) {
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let mut view = s.doc["view"].clone();
    view["id"] = json!(s.view());
    view["tenant"] = json!(s.tenant);
    view["operations"] = json!(ops);
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema");
    for (uri, doc) in [
        (format!("/q/views/{}", s.view()), view),
        (format!("/q/schemas/{}", s.tenant), schema),
        (format!("/q/policy/{}", s.tenant), s.policy()),
    ] {
        let (status, _, body) = send(app, "PUT", &uri, &[], Some(&doc)).await;
        assert!(status.is_success(), "{}: {uri}: {status} {body}", s.name);
    }
}

/// The seed (the scenario's own `seed`, else `_seed.json`), written in the
/// Tenant of `s` as the seeder.
async fn seed(app: &Router, s: &Scenario) {
    let seed: Value = match s.doc.get("seed") {
        Some(own) => own.clone(),
        None => serde_json::from_str(SEED).expect("the seed"),
    };
    let uri = "/ngsi-ld/v1/entityOperations/upsert";
    let (status, _, body) = send(app, "POST", uri, &seeder(s), Some(&seed)).await;
    assert!(status.is_success(), "{}: seed: {status} {body}", s.name);
}

/// The Attribute names of an NGSI-LD Entity.
fn ngsi_names(e: &Value) -> BTreeSet<String> {
    e.as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k.clone())
        .filter(|k| !FRAME.contains(&k.as_str()))
        .collect()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// A next link as a path below the view root or the Tenant root.
fn local(href: &str) -> String {
    href.find("/x/views/")
        .or_else(|| href.find("/ngsi-ld/v1/"))
        .map_or(href, |i| &href[i..])
        .to_owned()
}

/// What one surface answered: the Entities read, the count it reported, or
/// a refusal with its status and body.
#[derive(Debug, Clone)]
enum Read {
    Entities(Answer, Option<u64>),
    Refused(StatusCode, Value),
}

/// Insert `id` with `names`, or say it came twice.
fn add(out: &mut Answer, id: String, names: BTreeSet<String>, twice: &mut Vec<String>) {
    if out.insert(id.clone(), names).is_some() {
        twice.push(id);
    }
}

/// NGSI-LD Query Entities through the view, following `Link: rel="next"`.
/// With `temporal`, Query Temporal Evolution of Entities (11.3.3) over
/// every instance since 2000 instead.
async fn ngsi_ld(app: &Router, s: &Scenario, temporal: bool) -> Read {
    let types = format!("type={}", s.types().join(","));
    if temporal {
        let window = "&timerel=after&timeAt=2000-01-01T00:00:00Z&timeproperty=modifiedAt";
        ngsi_ld_read(app, s, "temporal/entities", &format!("{types}{window}")).await
    } else {
        ngsi_ld_read(app, s, "entities", &types).await
    }
}

/// NGSI-LD `path` below the view root with `query`, following
/// `Link: rel="next"`.
async fn ngsi_ld_read(app: &Router, s: &Scenario, path: &str, query: &str) -> Read {
    let uri = format!(
        "/x/views/{}/ngsi-ld/v1/{path}?{query}&limit={PAGE}&count=true",
        s.view()
    );
    paged(app, &with_tenant(s), uri).await
}

/// NGSI-LD `uri` as `who`, following `Link: rel="next"`.
async fn paged(app: &Router, who: &[(String, String)], mut uri: String) -> Read {
    let (mut out, mut twice, mut count) = (Answer::new(), Vec::new(), None);
    let mut pages = 0;
    loop {
        pages += 1;
        if pages > MAX_PAGES {
            return Read::Refused(StatusCode::LOOP_DETECTED, json!("a next link without end"));
        }
        let (status, headers, body) = send(app, "GET", &uri, who, None).await;
        if status != StatusCode::OK {
            return Read::Refused(status, body);
        }
        if count.is_none() {
            count = headers
                .get("NGSILD-Results-Count")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.parse().ok());
        }
        for e in body.as_array().into_iter().flatten() {
            add(&mut out, text(e, "id"), ngsi_names(e), &mut twice);
        }
        let next = headers
            .get_all("Link")
            .iter()
            .filter_map(|h| h.to_str().ok())
            .find(|l| l.contains("rel=\"next\""))
            .and_then(|l| Some(l[l.find('<')? + 1..l.find('>')?].to_owned()));
        match next {
            Some(n) => uri = local(&n),
            None => break,
        }
    }
    if !twice.is_empty() {
        out.insert(format!("twice: {twice:?}"), BTreeSet::new());
    }
    Read::Entities(out, count)
}

/// The OData EntitySet of each type, following `@odata.nextLink`; a set
/// the view does not serve (404) holds nothing.
async fn odata(app: &Router, s: &Scenario) -> Read {
    odata_read(app, s, &s.types(), "").await
}

/// The OData EntitySets of `types` with the options `extra`, following
/// `@odata.nextLink`; a set the view does not serve (404) holds nothing, and
/// one it refuses (403) too, as a read naming several types leaves a refused
/// one out (T-4455): refused only when every set is.
async fn odata_read(app: &Router, s: &Scenario, types: &[String], extra: &str) -> Read {
    let who = with_tenant(s);
    let (mut out, mut twice, mut total) = (Answer::new(), Vec::new(), 0u64);
    let mut refused = Vec::new();
    for t in types {
        let mut uri = format!("/x/views/{}/odata/v4/{t}?$count=true{extra}", s.view());
        let mut first = true;
        let mut paged = who.clone();
        paged.push(("Prefer".to_owned(), format!("odata.maxpagesize={PAGE}")));
        let mut pages = 0;
        loop {
            pages += 1;
            if pages > MAX_PAGES {
                return Read::Refused(StatusCode::LOOP_DETECTED, json!("a next link without end"));
            }
            let (status, _, body) = send(app, "GET", &uri, &paged, None).await;
            if status == StatusCode::NOT_FOUND && first {
                break;
            }
            if status == StatusCode::FORBIDDEN && first {
                refused.push(Read::Refused(status, body));
                break;
            }
            if status != StatusCode::OK {
                return Read::Refused(status, body);
            }
            if first {
                total += body
                    .get("@odata.count")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX / 4);
                first = false;
            }
            for row in body
                .get("value")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let names = row
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(k, v)| {
                        !k.contains('@')
                            && !matches!(k.as_str(), "id" | "_types" | "scope")
                            && !v.is_null()
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
                add(&mut out, text(row, "id"), names, &mut twice);
            }
            match body.get("@odata.nextLink").and_then(Value::as_str) {
                Some(next) => uri = local(next),
                None => break,
            }
        }
    }
    if !types.is_empty() && refused.len() == types.len() {
        if let Some(refusal) = refused.pop() {
            return refusal;
        }
    }
    if !twice.is_empty() {
        out.insert(format!("twice: {twice:?}"), BTreeSet::new());
    }
    Read::Entities(out, Some(total))
}

/// OGC API Features items of each collection, following `rel=next`; a
/// collection the view does not serve (404) holds nothing. The geometry is
/// `location`.
async fn ogc(app: &Router, s: &Scenario) -> Read {
    ogc_read(app, s, &s.types(), "").await
}

/// OGC API Features items of the collections `types` with the parameters
/// `extra`, following `rel=next`; a collection the view does not serve
/// (404) holds nothing, and one it refuses (403) too, as a read naming
/// several types leaves a refused one out (T-4455): refused only when every
/// collection is.
async fn ogc_read(app: &Router, s: &Scenario, types: &[String], extra: &str) -> Read {
    let who = with_tenant(s);
    let (mut out, mut twice, mut total) = (Answer::new(), Vec::new(), 0u64);
    let mut refused = Vec::new();
    for t in types {
        let mut uri = format!(
            "/x/views/{}/ogc/features/collections/{t}/items?limit={PAGE}{extra}",
            s.view()
        );
        let mut first = true;
        let mut pages = 0;
        loop {
            pages += 1;
            if pages > MAX_PAGES {
                return Read::Refused(StatusCode::LOOP_DETECTED, json!("a next link without end"));
            }
            let (status, _, body) = send(app, "GET", &uri, &who, None).await;
            if status == StatusCode::NOT_FOUND && first {
                break;
            }
            if status == StatusCode::FORBIDDEN && first {
                refused.push(Read::Refused(status, body));
                break;
            }
            if status != StatusCode::OK {
                return Read::Refused(status, body);
            }
            if first {
                total += body
                    .get("numberMatched")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX / 4);
                first = false;
            }
            for f in body
                .get("features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let mut names: BTreeSet<String> = f
                    .get("properties")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, _)| k.clone())
                    .collect();
                if f.get("geometry").is_some_and(|g| !g.is_null()) {
                    names.insert("location".to_owned());
                }
                add(&mut out, text(f, "id"), names, &mut twice);
            }
            let next = body
                .get("links")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|l| l["rel"] == "next")
                .and_then(|l| l["href"].as_str())
                .map(local);
            match next {
                Some(n) => uri = n,
                None => break,
            }
        }
    }
    if !types.is_empty() && refused.len() == types.len() {
        if let Some(refusal) = refused.pop() {
            return refusal;
        }
    }
    if !twice.is_empty() {
        out.insert(format!("twice: {twice:?}"), BTreeSet::new());
    }
    Read::Entities(out, Some(total))
}

/// One MCP `tools/call` through the view; the JSON-RPC answer.
async fn mcp_call(app: &Router, s: &Scenario, tool: &str, args: Value) -> Value {
    const MODERN: &str = "2026-07-28";
    let params = json!({"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "views-matrix", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    }});
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
    let mut who = with_tenant(s);
    for (k, v) in [
        ("Accept", "application/json, text/event-stream"),
        ("MCP-Protocol-Version", MODERN),
        ("Mcp-Method", "tools/call"),
        ("Mcp-Name", tool),
    ] {
        who.push((k.to_owned(), v.to_owned()));
    }
    let uri = format!("/x/views/{}/mcp", s.view());
    send(app, "POST", &uri, &who, Some(&body)).await.2
}

/// MCP `query_entities`, cursor by cursor; an `isError` result is a refusal.
async fn mcp(app: &Router, s: &Scenario) -> Read {
    let (mut out, mut twice, mut total) = (Answer::new(), Vec::new(), None);
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        if pages > MAX_PAGES {
            return Read::Refused(StatusCode::LOOP_DETECTED, json!("a next link without end"));
        }
        let mut args = json!({"type": s.types(), "limit": PAGE, "count": true});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let answer = mcp_call(app, s, "query_entities", args).await;
        let Some(page) = answer
            .pointer("/result/structuredContent")
            .filter(|_| answer.pointer("/result/isError") != Some(&json!(true)))
        else {
            return Read::Refused(StatusCode::FORBIDDEN, answer);
        };
        if total.is_none() {
            total = page.get("total").and_then(Value::as_u64);
        }
        for e in page
            .get("entities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            add(&mut out, text(e, "id"), ngsi_names(e), &mut twice);
        }
        cursor = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    if !twice.is_empty() {
        out.insert(format!("twice: {twice:?}"), BTreeSet::new());
    }
    Read::Entities(out, total)
}

fn with_tenant(s: &Scenario) -> Vec<(String, String)> {
    let mut who = s.caller();
    who.push(tenant_header(s));
    who
}

/// The self URL the internal CSR row registers this broker by.
#[cfg(feature = "internal-csr")]
const SELF_URL: &str = "http://self.invalid";

/// The Tenant whose registration reads the view of `s` in process.
#[cfg(feature = "internal-csr")]
fn source(s: &Scenario) -> String {
    format!("{}-src", s.tenant)
}

/// Query Entities in the source Tenant of `s` as its caller, every page:
/// the source holds no Entity of its own and one internal registration
/// (ADR-0025) of this broker for the seed types in the Tenant of `s`, under
/// that Tenant's grant naming the view of `s`. So the forward is the whole
/// answer, decided by the view and its engine for the caller who asked.
#[cfg(feature = "internal-csr")]
async fn forward(app: &Router, s: &Scenario) -> Read {
    let src = source(s);
    let grant = json!({"target": s.tenant, "source": src, "view": s.view()});
    // the source's own engine lets the caller query, whoever it is, and the
    // seeder register: only the target's view and engine decide the answer
    let mut policies: Vec<Value> = s
        .caller()
        .iter()
        .flat_map(|(header, v)| {
            let kind = match header.as_str() {
                "x-jc-subject" => "user",
                "x-jc-client" => "serviceAccount",
                "x-jc-groups" => "group",
                _ => "role",
            };
            v.split(',').map(move |id| (kind, id.trim().to_owned()))
        })
        .enumerate()
        .map(|(n, (kind, id))| {
            json!({"id": format!("caller-{n}"), "assignee": {"kind": kind, "id": id},
                   "operations": READS})
        })
        .collect();
    policies.extend([
        // an anonymous caller holds role public alone, whatever it claims
        json!({"id": "anonymous", "assignee": {"kind": "role", "id": "public"},
               "operations": READS}),
        json!({"id": "matrix-seed", "assignee": {"kind": "serviceAccount", "id": SEEDER},
               "operations": ["5.9.2"]}),
    ]);
    let policy = json!({ "policies": policies });
    for (uri, doc) in [
        (format!("/q/grants/{}/{src}", s.tenant), grant),
        (format!("/q/policy/{src}"), policy),
    ] {
        let (status, _, body) = send(app, "PUT", &uri, &[], Some(&doc)).await;
        assert!(status.is_success(), "{}: {uri}: {status} {body}", s.name);
    }
    let registration = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:matrix",
        "type": "ContextSourceRegistration",
        "endpoint": SELF_URL,
        "tenant": s.tenant,
        "information": [{"entities": TYPES.map(|t| json!({"type": t}))}],
    });
    let seeder = vec![
        ("x-jc-client".to_owned(), SEEDER.to_owned()),
        ("NGSILD-Tenant".to_owned(), src.clone()),
    ];
    let uri = "/ngsi-ld/v1/csourceRegistrations";
    let (status, _, body) = send(app, "POST", uri, &seeder, Some(&registration)).await;
    // the differential's cases share one source Tenant and its registration
    assert!(
        status == StatusCode::CREATED || status == StatusCode::CONFLICT,
        "{}: the registration: {status} {body}",
        s.name
    );
    let mut who = s.caller();
    who.push(("NGSILD-Tenant".to_owned(), src));
    let uri = format!(
        "/ngsi-ld/v1/entities?type={}&limit={PAGE}&count=true",
        s.types().join(",")
    );
    paged(app, &who, uri).await
}

/// What a forward owes `want`: a denied caller gets the answer of no
/// registration, nothing admitted (the merge reports the refusal as a
/// warning, ADR-0025 C10).
#[cfg(feature = "internal-csr")]
fn forwarded(want: &Expect) -> Expect {
    match want {
        Expect::Denied => Expect::Admitted(Answer::new()),
        admitted => admitted.clone(),
    }
}

/// Every seed id, retrieved one by one through the NGSI-LD view and MCP
/// `get_entity`: an admitted one with its Attributes, any other with the
/// answer an absent id gets (its own id masked).
async fn by_id(app: &Router, s: &Scenario, want: &Answer, failures: &mut Vec<String>) {
    let seed: Value = serde_json::from_str(SEED).expect("the seed");
    let who = with_tenant(s);
    let absent = "urn:ngsi-ld:Vehicle:absent";
    let masked = |body: &Value, id: &str| body.to_string().replace(id, "ID");
    let root = format!("/x/views/{}/ngsi-ld/v1/entities", s.view());
    let (_, _, nothing) = send(app, "GET", &format!("{root}/{absent}"), &who, None).await;
    let mcp_nothing = mcp_call(app, s, "get_entity", json!({"id": absent})).await;
    let refused = s.doc["byId"] == "denied";
    for e in seed.as_array().into_iter().flatten() {
        let id = text(e, "id");
        let (status, _, body) = send(app, "GET", &format!("{root}/{id}"), &who, None).await;
        let answer = mcp_call(app, s, "get_entity", json!({"id": id})).await;
        if refused {
            if status != StatusCode::FORBIDDEN || masked(&body, &id) != masked(&nothing, absent) {
                failures.push(format!(
                    "NGSI-LD retrieve {id} refused: {status} {body}, an absent id gets {nothing}"
                ));
            }
            if masked(&answer, &id) != masked(&mcp_nothing, absent) {
                failures.push(format!(
                    "MCP get_entity {id} refused: {answer}, an absent id gets {mcp_nothing}"
                ));
            }
            continue;
        }
        let mcp_entity = answer
            .pointer("/result/structuredContent/entity")
            .filter(|_| answer.pointer("/result/isError") != Some(&json!(true)));
        match want.get(&id) {
            Some(names) => {
                if status != StatusCode::OK || &ngsi_names(&body) != names {
                    failures.push(format!("NGSI-LD retrieve {id}: {status} {body}"));
                }
                if mcp_entity.map(ngsi_names).as_ref() != Some(names) {
                    failures.push(format!("MCP get_entity {id}: {answer}"));
                }
            }
            None => {
                if status != StatusCode::NOT_FOUND || masked(&body, &id) != masked(&nothing, absent)
                {
                    failures.push(format!(
                        "NGSI-LD retrieve {id} not admitted: {status} {body}, an absent id gets {nothing}"
                    ));
                }
                if mcp_entity.is_some() || masked(&answer, &id) != masked(&mcp_nothing, absent) {
                    failures.push(format!(
                        "MCP get_entity {id} not admitted: {answer}, an absent id gets {mcp_nothing}"
                    ));
                }
            }
        }
    }
}

/// `read` against `want` on the surface `surface`.
fn judge(surface: &str, read: Read, want: &Expect, failures: &mut Vec<String>) {
    match (read, want) {
        (Read::Entities(got, count), Expect::Admitted(want)) => {
            if &got != want {
                failures.push(format!("{surface}: got {got:?}, want {want:?}"));
            }
            if count != u64::try_from(want.len()).ok() {
                failures.push(format!("{surface}: count {count:?}, want {}", want.len()));
            }
        }
        (Read::Refused(status, body), Expect::Admitted(_)) => {
            failures.push(format!("{surface}: refused {status} {body}"));
        }
        (Read::Entities(got, count), Expect::Denied) => {
            failures.push(format!(
                "{surface}: denied caller read {got:?} (count {count:?})"
            ));
        }
        (Read::Refused(status, body), Expect::Denied) => {
            let text = body.to_string();
            // a refusal names no Entity: no URN of the seed or of a generated case
            if status.is_success() || text.contains("urn:ngsi-ld:") {
                failures.push(format!("{surface}: refusal {status} {text}"));
            }
        }
    }
}

/// Judge `got` against what NGSI-LD answered for the same question: the
/// same Entities and names, or a refusal on both. A surface may refuse a
/// name the view hides (`400`) where NGSI-LD answers nothing.
fn same(surface: &str, got: Read, ngsi: Read, hidden: bool, failures: &mut Vec<String>) {
    let want = match ngsi {
        Read::Entities(want, _) => {
            if let Read::Refused(StatusCode::BAD_REQUEST, _) = got {
                if hidden || want.is_empty() {
                    return judge(surface, got, &Expect::Denied, failures);
                }
            }
            Expect::Admitted(want)
        }
        Read::Refused(..) => Expect::Denied,
    };
    judge(surface, got, &want, failures);
}

/// Whether one of `names` is shown on no Vehicle `want` admits: a surface
/// refuses a name the view hides like a misspelt one (`400`, ogc.md 2,
/// odata.md), where NGSI-LD omits it.
fn hidden(want: &Expect, names: &[&str]) -> bool {
    let Expect::Admitted(want) = want else {
        return false;
    };
    names.iter().any(|n| {
        !want
            .iter()
            .any(|(id, shown)| id.contains(":Vehicle:") && shown.contains(*n))
    })
}

/// The query-language rows, each through the same view and caller: OData
/// `$filter` and `$select` against NGSI-LD `q` and `pick`, OGC `bbox` and
/// `properties` against a `geoQ` intersects and `pick` plus the Features
/// without a geometry, which `bbox` keeps (Part 1 Req 24 C). The box has
/// V2 on its corner.
async fn rows(app: &Router, s: &Scenario, want: &Expect, failures: &mut Vec<String>) {
    let vehicles = ["Vehicle".to_owned()];
    let pick = "type=Vehicle&pick=id,type,location,speed";
    let ngsi = ngsi_ld_read(app, s, "entities", &format!("{pick}&q=speed%3E40")).await;
    let got = odata_read(
        app,
        s,
        &vehicles,
        "&$filter=speed%20gt%2040&$select=location,speed",
    )
    .await;
    same(
        "OData $filter $select",
        got,
        ngsi,
        hidden(want, &["location", "speed"]),
        failures,
    );
    let geo = "&georel=intersects&geometry=Polygon&coordinates=%5B%5B%5B0,0%5D,%5B7,0%5D,%5B7,7%5D,%5B0,7%5D,%5B0,0%5D%5D%5D";
    let mut ngsi = ngsi_ld_read(app, s, "entities", &format!("{pick}{geo}")).await;
    if let (Read::Entities(inside, n), Read::Entities(all, _)) =
        (&mut ngsi, ngsi_ld_read(app, s, "entities", pick).await)
    {
        inside.extend(
            all.into_iter()
                .filter(|(_, names)| !names.contains("location")),
        );
        *n = u64::try_from(inside.len()).ok();
    }
    let got = ogc_read(app, s, &vehicles, "&bbox=0,0,7,7&properties=speed").await;
    same(
        "OGC bbox properties",
        got,
        ngsi,
        hidden(want, &["speed"]),
        failures,
    );
}

/// The LinkML class of each type through the view (`classes/{class}`):
/// the type's name with its slot names, a class the caller may not see
/// (`404`, LK1) holding nothing.
#[cfg(feature = "linkml")]
async fn linkml(app: &Router, s: &Scenario) -> Read {
    let mut out = Answer::new();
    for t in TYPES {
        let uri = format!("/x/views/{}/linkml/classes/{t}", s.view());
        let (status, _, body) = send(app, "GET", &uri, &with_tenant(s), None).await;
        match status {
            StatusCode::OK => {
                let slots = body["slots"].as_array().into_iter().flatten();
                out.insert(t.to_owned(), slots.map(|slot| text(slot, "name")).collect());
            }
            StatusCode::NOT_FOUND => {}
            _ => return Read::Refused(status, body),
        }
    }
    let n = u64::try_from(out.len()).ok();
    Read::Entities(out, n)
}

/// The classes LinkML serves where NGSI-LD answers `want`: each admitted
/// type with the names its admitted Entities show, unless the scenario
/// states `linkml` (a rule's condition is not read, linkml.md 2, so a
/// class whose Entities a condition drops still shows). A denied caller
/// sees no class.
#[cfg(feature = "linkml")]
fn classes(want: &Expect, stated: &Value) -> Expect {
    if let Some(stated) = stated.as_object() {
        return expect(&Value::Object(stated.clone()));
    }
    let mut out = Answer::new();
    if let Expect::Admitted(want) = want {
        for (id, names) in want {
            let t = id.split(':').nth(2).unwrap_or_default().to_owned();
            out.entry(t).or_default().extend(names.iter().cloned());
        }
    }
    Expect::Admitted(out)
}

/// What a temporal query answers where the snapshot answers `want`: an
/// Entity shows in it only through an instance of a shown Attribute, so one
/// shown with its frame alone is absent (ADR-0023 rule 11, temporal);
/// `temporal` in the scenario or step states it where a move changed the
/// history.
fn temporal(want: &Expect, stated: &Value) -> Expect {
    match (want, stated.is_null()) {
        (_, false) => expect(stated),
        (Expect::Admitted(want), true) => Expect::Admitted(
            want.iter()
                .filter(|(_, names)| !names.is_empty())
                .map(|(id, names)| (id.clone(), names.clone()))
                .collect(),
        ),
        (Expect::Denied, true) => Expect::Denied,
    }
}

/// Every surface against `want`; `stated` is the scenario's or step's
/// `temporal`.
async fn check(
    app: &Router,
    s: &Scenario,
    want: &Expect,
    stated: &Value,
    failures: &mut Vec<String>,
) {
    judge(
        "NGSI-LD query",
        ngsi_ld(app, s, false).await,
        want,
        failures,
    );
    judge(
        "NGSI-LD temporal",
        ngsi_ld(app, s, true).await,
        &temporal(want, stated),
        failures,
    );
    judge("OData", odata(app, s).await, want, failures);
    judge("OGC items", ogc(app, s).await, want, failures);
    judge("MCP query_entities", mcp(app, s).await, want, failures);
    if let Expect::Admitted(want) = want {
        by_id(app, s, want, failures).await;
    }
}

/// A step: `set` writes on the Tenant root as the seeder, `patch` writes as
/// the caller through the view and expects `status`.
async fn step(app: &Router, s: &Scenario, step: &Value, failures: &mut Vec<String>) {
    let attrs = &step["attrs"];
    if let Some(id) = step["set"].as_str() {
        let uri = format!("/ngsi-ld/v1/entities/{id}/attrs");
        let (status, _, body) = send(app, "PATCH", &uri, &seeder(s), Some(attrs)).await;
        assert!(status.is_success(), "{}: set {id}: {status} {body}", s.name);
    }
    if let Some(id) = step["patch"].as_str() {
        let uri = format!("/x/views/{}/ngsi-ld/v1/entities/{id}/attrs", s.view());
        let (status, _, body) = send(app, "PATCH", &uri, &with_tenant(s), Some(attrs)).await;
        if Some(u64::from(status.as_u16())) != step["status"].as_u64() {
            failures.push(format!(
                "write {id}: {status} {body}, want {}",
                step["status"]
            ));
        }
    }
}

/// A WebSocket Subscription through the view, made as the caller before
/// the seed is written: its status, and the socket when it was made.
async fn subscribe(port: u16, s: &Scenario) -> (u64, Option<Socket>) {
    let (path, mut who) = (format!("/x/views/{}/ws", s.view()), with_tenant(s));
    who.push(("l5d-client-id".to_owned(), GATEWAY.to_owned()));
    let types: Vec<Value> = s.types().iter().map(|t| json!({"type": t})).collect();
    let name = s.name.clone();
    tokio::task::spawn_blocking(move || {
        let mut socket = Socket::open(port, &path, &who, Duration::from_millis(800))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let welcome = socket.next().unwrap_or_default();
        assert_eq!(
            welcome["metadata"]["x-op"], "x-welcome",
            "{name}: {welcome}"
        );
        socket
            .send(&json!({"metadata": {"x-op": "x-subscribe", "x-id": "m"},
                          "body": {"type": "Subscription", "entities": types}}))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let made = socket.next().unwrap_or_default();
        let status = made["metadata"]["x-status"].as_u64().unwrap_or_default();
        (status, (status == 201).then_some(socket))
    })
    .await
    .expect("a subscription")
}

/// The HTTP endpoint every Subscription notifies: each notification with
/// the path it was sent to.
async fn receiver() -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}/n/", listener.local_addr().expect("an address"));
    let hooks = Router::new().route(
        "/n/{tenant}",
        axum::routing::post(
            move |axum::extract::Path(tenant): axum::extract::Path<String>,
                  body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    let v = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let _ = tx.send((tenant, v));
                    StatusCode::NO_CONTENT
                }
            },
        ),
    );
    tokio::spawn(async move { axum::serve(listener, hooks).await });
    (base, rx)
}

/// An HTTP Subscription through the view's NGSI-LD root, made as the caller
/// before the seed is written, notifying `{base}{tenant}`: its status.
async fn subscribe_http(app: &Router, s: &Scenario, base: &str) -> StatusCode {
    let types: Vec<Value> = s.types().iter().map(|t| json!({"type": t})).collect();
    let sub = json!({"type": "Subscription", "entities": types,
                     "notification": {"endpoint": {"uri": format!("{base}{}", s.tenant),
                                                   "accept": "application/json"}}});
    let uri = format!("/x/views/{}/ngsi-ld/v1/subscriptions", s.view());
    send(app, "POST", &uri, &with_tenant(s), Some(&sub)).await.0
}

/// Every Entity notified to the Tenant of `s` until the endpoint fell
/// silent, the last notification of each id.
async fn notified_http(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    s: &Scenario,
) -> Read {
    let mut out = Answer::new();
    while let Ok(Some((tenant, n))) =
        tokio::time::timeout(Duration::from_millis(800), rx.recv()).await
    {
        // an earlier scenario's steps may still notify its own Tenant
        if tenant == s.tenant {
            for e in n["data"].as_array().into_iter().flatten() {
                out.insert(text(e, "id"), ngsi_names(e));
            }
        }
    }
    let n = u64::try_from(out.len()).ok();
    Read::Entities(out, n)
}

/// Every Entity the socket was notified until it fell silent, the last
/// notification of each id.
async fn notified(socket: Socket) -> Read {
    tokio::task::spawn_blocking(move || {
        let mut socket = socket;
        let (mut out, mut never) = (Answer::new(), Vec::new());
        while let Some(m) = socket.next() {
            if m["metadata"]["x-op"] == "x-notification" {
                for e in m["body"]["data"].as_array().into_iter().flatten() {
                    out.insert(text(e, "id"), ngsi_names(e));
                }
            } else {
                never.push(m);
            }
        }
        if !never.is_empty() {
            out.insert(format!("other messages: {never:?}"), BTreeSet::new());
        }
        let n = u64::try_from(out.len()).ok();
        Read::Entities(out, n)
    })
    .await
    .expect("the notifications")
}

/// The whole corpus through every surface; every mismatch is listed with
/// its scenario.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_surface_answers_what_ngsi_ld_admits_in_every_scenario() {
    let (app, _reload) = broker().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("an address").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    antares_jsonld::allow_private_egress(true);
    let (hooks, mut hooked) = receiver().await;
    let corpus = corpus();
    assert!(
        corpus.len() >= 40,
        "the corpus has {} scenarios",
        corpus.len()
    );
    let (mut report, mut listed) = (Vec::new(), Vec::new());
    for s in &corpus {
        install(&app, s).await;
        let want = expect(&s.doc["expect"]);
        let mut failures = Vec::new();
        // the Subscription is made before the seed, so the seed notifies it
        let (status, socket) = subscribe(port, s).await;
        let made = subscribe_http(&app, s, &hooks).await;
        seed(&app, s).await;
        // a notification carries what the view shows now; an Entity shown
        // with its frame alone has nothing to notify
        let shown = temporal(&want, &Value::Null);
        match (made, &want) {
            (StatusCode::CREATED, _) => {
                judge(
                    "HTTP notification",
                    notified_http(&mut hooked, s).await,
                    &shown,
                    &mut failures,
                );
            }
            (_, Expect::Denied) => {}
            (_, Expect::Admitted(_)) => {
                failures.push(format!(
                    "HTTP notification: the Subscription answered {made}"
                ));
            }
        }
        match (socket, &want) {
            (Some(socket), _) => {
                judge("WebSocket", notified(socket).await, &shown, &mut failures);
            }
            (None, Expect::Denied) => {}
            (None, Expect::Admitted(_)) => {
                failures.push(format!("WebSocket: the Subscription answered {status}"));
            }
        }
        check(&app, s, &want, &s.doc["temporal"], &mut failures).await;
        rows(&app, s, &want, &mut failures).await;
        #[cfg(feature = "internal-csr")]
        judge(
            "internal CSR",
            forward(&app, s).await,
            &forwarded(&want),
            &mut failures,
        );
        #[cfg(feature = "linkml")]
        judge(
            "LinkML",
            linkml(&app, s).await,
            &classes(&want, &s.doc["linkml"]),
            &mut failures,
        );
        for (n, st) in s.doc["steps"].as_array().into_iter().flatten().enumerate() {
            let mut after = Vec::new();
            step(&app, s, st, &mut after).await;
            check(&app, s, &expect(&st["expect"]), &st["temporal"], &mut after).await;
            failures.extend(after.into_iter().map(|f| format!("step {n}: {f}")));
        }
        // A mismatch with a fix task filed is listed in the scenario's
        // `known` by surface; it must still fail, so a fix removes the entry.
        let known: BTreeMap<String, String> = s.doc["known"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
            .collect();
        let of =
            |f: &str, surface: &str| f.starts_with(surface) || f.contains(&format!(": {surface}"));
        for (surface, task) in &known {
            // a surface this build does not serve has nothing to match
            if (surface == "LinkML" && !cfg!(feature = "linkml"))
                || (surface == "internal CSR" && !cfg!(feature = "internal-csr"))
            {
                continue;
            }
            let hits = failures.iter().filter(|f| of(f, surface)).count();
            if hits == 0 {
                report.push(format!(
                    "{}: {surface} matches now; remove its `known` entry ({task})",
                    s.name
                ));
            } else {
                listed.push(format!("{} {surface}: {hits} mismatches, {task}", s.name));
            }
        }
        failures.retain(|f| !known.keys().any(|surface| of(f, surface)));
        report.extend(failures.into_iter().map(|f| format!("{}: {f}", s.name)));
    }
    eprintln!(
        "known mismatches, each with its fix task:\n{}",
        listed.join("\n")
    );
    assert!(
        report.is_empty(),
        "{} mismatches:\n{}",
        report.len(),
        report.join("\n")
    );
}

// The differential (T-3777 step 3): generated Entities, a generated view and
// up to three generated policies of the corpus's grammar, each case in a
// Tenant of its own, and every surface must answer what NGSI-LD answers
// through the same view for the same caller. `VIEWS_DIFF_CASES` sets the
// number of cases (200; ci runs 2 000), `VIEWS_DIFF_SEED` the seed, which
// the test prints. A failure shrinks and prints the minimal case as a
// corpus file with its own `seed`; its `expect` is then written by hand.

/// The areas a view or policy names, `[x0, y0, x1, y1]`; the generated
/// points lie on a 0..=12 grid, so some fall on an edge.
const AREAS: [[u8; 4]; 3] = [[0, 0, 6, 6], [4, 4, 12, 12], [8, 0, 12, 4]];
const SCOPES: [&str; 3] = ["/Zilina/transport", "/Zilina/police", "/Kosice/transport"];
const SCOPE_QS: [&str; 4] = [
    "/Zilina/transport",
    "/Zilina/#",
    "/+/transport",
    "/Kosice/transport",
];
const QS: [&str; 7] = [
    "speed>40",
    "speed<50",
    "temperature>20",
    "operator==\"city\"",
    "!operator",
    "vehicleType==\"car\"",
    "speed>20;operator==\"city\"",
];
const NAMES: [&str; 5] = [
    "location",
    "operator",
    "speed",
    "temperature",
    "vehicleType",
];

fn ring(area: usize) -> Value {
    let [x0, y0, x1, y1] = AREAS[area % AREAS.len()];
    json!([[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]])
}

fn georel(within: bool) -> &'static str {
    if within {
        "within"
    } else {
        "intersects"
    }
}

/// Up to eight Vehicles and Sensors, each Attribute present or not.
fn generated_seed() -> impl Strategy<Value = Vec<Value>> {
    let one = (
        any::<bool>(),
        proptest::option::of((0u8..13, 0u8..13)),
        proptest::option::of(0u8..100),
        proptest::option::of(any::<bool>()),
        proptest::option::of(any::<bool>()),
        proptest::option::of(0..SCOPES.len()),
    );
    proptest::collection::vec(one, 1..8).prop_map(|all| {
        all.into_iter()
            .enumerate()
            .map(|(i, (vehicle, at, n, city, car, scope))| {
                let t = if vehicle { "Vehicle" } else { "Sensor" };
                let prop = |v: Value| json!({"type": "Property", "value": v});
                let mut e = json!({"id": format!("urn:ngsi-ld:{t}:G{i}"), "type": t});
                if let Some((x, y)) = at {
                    e["location"] = json!({"type": "GeoProperty",
                                           "value": {"type": "Point", "coordinates": [x, y]}});
                }
                if let Some(n) = n {
                    e[if vehicle { "speed" } else { "temperature" }] = prop(json!(n));
                }
                if let Some(city) = city {
                    e["operator"] = prop(json!(if city { "city" } else { "bus" }));
                }
                if let (true, Some(car)) = (vehicle, car) {
                    e["vehicleType"] = prop(json!(if car { "car" } else { "bus" }));
                }
                if let Some(scope) = scope {
                    e["scope"] = json!(SCOPES[scope]);
                }
                e
            })
            .collect()
    })
}

/// A condition: a `q`, an area with its georel, a `scopeQ`, each or not.
type Condition = (Option<usize>, Option<(usize, bool)>, Option<usize>);

fn condition() -> impl Strategy<Value = Condition> {
    (
        proptest::option::of(0..QS.len()),
        proptest::option::of((0..AREAS.len(), any::<bool>())),
        proptest::option::of(0..SCOPE_QS.len()),
    )
}

/// The types a generated view or policy names: Vehicle, Sensor or both.
fn types(which: usize) -> Vec<&'static str> {
    match which % 3 {
        0 => vec!["Vehicle"],
        1 => vec!["Sensor"],
        _ => vec!["Vehicle", "Sensor"],
    }
}

/// A view of `*`, or of one or both types with a projection, and a
/// condition on the view.
fn generated_view() -> impl Strategy<Value = Value> {
    (
        0usize..4,
        proptest::sample::subsequence(NAMES.to_vec(), 1..=NAMES.len()),
        condition(),
    )
        .prop_map(|(shape, names, (q, area, scope))| {
            let mut view = if shape == 3 {
                json!({"selector": ["*"], "rules": [{"types": ["*"], "attributes": ["*"]}]})
            } else {
                json!({"selector": types(shape),
                       "rules": [{"types": types(shape), "attributes": names}]})
            };
            if let Some(q) = q {
                view["q"] = json!(QS[q]);
            }
            if let Some((area, within)) = area {
                view["geoQ"] = json!({"georel": georel(within), "geometry": "Polygon",
                                      "coordinates": ring(area)});
            }
            if let Some(scope) = scope {
                view["scopeQ"] = json!(SCOPE_QS[scope]);
            }
            view
        })
}

/// No policy (the public role reads everything), or one to three for the
/// caller: a type or both, a projection or the whole Entity, a condition,
/// one in four a prohibition.
fn generated_policies() -> impl Strategy<Value = Option<Vec<Value>>> {
    let one = (
        0usize..3,
        proptest::option::of(proptest::sample::subsequence(
            NAMES.to_vec(),
            1..=NAMES.len(),
        )),
        condition(),
        0u8..4,
    );
    proptest::option::of(proptest::collection::vec(one, 1..=3)).prop_map(|all| {
        all.map(|all| {
            all.into_iter()
                .enumerate()
                .map(|(i, (which, names, (q, area, scope), effect))| {
                    let entities: Vec<Value> =
                        types(which).iter().map(|t| json!({"type": t})).collect();
                    let mut information = json!({"entities": entities});
                    if let Some(names) = names {
                        information["propertyNames"] = json!(names);
                    }
                    let mut p = json!({"id": format!("p{i}"),
                                       "assignee": {"kind": "user", "id": "alice"},
                                       "operations": READS, "information": [information]});
                    if let Some(q) = q {
                        p["q"] = json!(QS[q]);
                    }
                    if let Some((area, within)) = area {
                        p["geoQ"] = json!(format!(
                            "georel={};geometry=Polygon;coordinates={}",
                            georel(within),
                            ring(area)
                        ));
                    }
                    if let Some(scope) = scope {
                        p["scopeQ"] = json!(SCOPE_QS[scope]);
                    }
                    if effect == 0 {
                        p["effect"] = json!("prohibit");
                    }
                    p
                })
                .collect()
        })
    })
}

/// Every surface of a generated case against NGSI-LD through the view. The
/// cases share one Tenant and view, each replacing the last, so the broker
/// does not grow with the number of cases; the seed is deleted after.
async fn differ(app: &Router, s: &Scenario) -> Vec<String> {
    install(app, s).await;
    seed(app, s).await;
    let failures = compare(app, s).await;
    let ids: Vec<Value> = s.doc["seed"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| e["id"].clone())
        .collect();
    let uri = "/ngsi-ld/v1/entityOperations/delete";
    let (status, _, body) = send(app, "POST", uri, &seeder(s), Some(&json!(ids))).await;
    assert!(status.is_success(), "{}: delete: {status} {body}", s.name);
    failures
}

async fn compare(app: &Router, s: &Scenario) -> Vec<String> {
    let ngsi = ngsi_ld(app, s, false).await;
    let mut failures = Vec::new();
    for (surface, read) in [
        ("OData", odata(app, s).await),
        ("OGC", ogc(app, s).await),
        ("MCP", mcp(app, s).await),
    ] {
        same(surface, read, ngsi.clone(), false, &mut failures);
    }
    #[cfg(feature = "internal-csr")]
    {
        let want = match ngsi {
            Read::Entities(want, _) => Expect::Admitted(want),
            Read::Refused(..) => Expect::Denied,
        };
        judge(
            "internal CSR",
            forward(app, s).await,
            &forwarded(&want),
            &mut failures,
        );
    }
    failures
}

/// Mismatches with a fix task filed: a failure of the surface whose text
/// holds the needle is printed with its task instead of failing the case.
const KNOWN_DIFF: [(&str, &str, &str); 0] = [];

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn every_surface_answers_what_ngsi_ld_admits_for_generated_views_and_policies() {
    let cases: u32 = env_or("VIEWS_DIFF_CASES", 200);
    let seed: u64 = env_or("VIEWS_DIFF_SEED", 3777);
    eprintln!("views differential: {cases} cases, VIEWS_DIFF_SEED={seed}");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let (app, _reload) = rt.block_on(broker());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut runner = TestRunner::new(ProptestConfig {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let strategy = (generated_seed(), generated_view(), generated_policies());
    let result = runner.run(&strategy, |(entities, view, policies)| {
        let n = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut doc = json!({"view": view, "caller": {"x-jc-subject": "alice"}, "seed": entities});
        if let Some(policies) = policies {
            doc["policies"] = json!(policies);
        }
        let s = Scenario {
            name: format!("generated {n}"),
            tenant: "g".to_owned(),
            doc,
        };
        let mut failures = rt.block_on(differ(&app, &s));
        failures.retain(|f| {
            let known = KNOWN_DIFF
                .iter()
                .find(|(surface, needle, _)| f.starts_with(surface) && f.contains(needle));
            if let Some((surface, _, task)) = known {
                eprintln!("{} {surface}: known, {task}", s.name);
            }
            known.is_none()
        });
        prop_assert!(
            failures.is_empty(),
            "{}\nthe case as a corpus file:\n{}",
            failures.join("\n"),
            serde_json::to_string_pretty(&s.doc).unwrap_or_default()
        );
        Ok(())
    });
    if let Err(e) = result {
        panic!(
            "{e}\nVIEWS_DIFF_SEED={seed}: add the minimal case as a corpus file, its `expect` \
             written by hand, and file its fix task"
        );
    }
}

/// T-4084, the dev shape: a public reader view's socket subscribes to Vehicle,
/// and the pipeline's writer view creates, then updates, the Vehicle by
/// batch upsert. Both writes reach the socket under the joinedcontext engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_through_a_writer_view_notifies_a_public_view_socket() {
    let (app, _reload) = broker().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("an address").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    let tenant = "hel";
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema");
    let policy = json!({"policies": [
        {"id": "public-transport", "assignee": {"kind": "role", "id": "public"},
         "operations": READS, "information": [{"entities": [{"type": "Vehicle"}]}]},
        {"id": "pipes", "assignee": {"kind": "serviceAccount", "id": SEEDER},
         "operations": ["5.6.1", "5.6.2", "5.6.3", "5.6.8", "5.6.9", "5.6.10"]}
    ], "views": {"transport": ["public-transport"], "pipes": ["pipes"]}});
    for (uri, doc) in [
        (
            "/q/views/transport".to_owned(),
            json!({"id": "transport", "tenant": tenant, "operations": ops,
                   "selector": ["Vehicle"], "rules": [{"types": ["Vehicle"], "attributes": ["speed"]}]}),
        ),
        (
            "/q/views/pipes".to_owned(),
            json!({"id": "pipes", "tenant": tenant, "operations": ops,
                   "selector": ["*"], "rules": [{"types": ["*"], "attributes": ["*"]}]}),
        ),
        (format!("/q/schemas/{tenant}"), schema),
        (format!("/q/policy/{tenant}"), policy),
    ] {
        let (status, _, body) = send(&app, "PUT", &uri, &[], Some(&doc)).await;
        assert!(status.is_success(), "{uri}: {status} {body}");
    }
    let socket = tokio::task::spawn_blocking(move || {
        let mut socket = Socket::open(
            port,
            "/x/views/transport/ws",
            &[("x-jc-roles".to_owned(), String::new())],
            Duration::from_millis(1500),
        )
        .expect("the socket");
        let welcome = socket.next().unwrap_or_default();
        assert_eq!(welcome["metadata"]["x-op"], "x-welcome", "{welcome}");
        socket
            .send(&json!({"metadata": {"x-op": "x-subscribe", "x-id": "s"},
                          "body": {"type": "Subscription", "entities": [{"type": "Vehicle"}]}}))
            .expect("sent");
        let made = socket.next().unwrap_or_default();
        assert_eq!(made["metadata"]["x-status"], 201, "{made}");
        socket
    })
    .await
    .expect("subscribed");
    let writer = [("x-jc-client".to_owned(), SEEDER.to_owned())];
    let uri = "/x/views/pipes/ngsi-ld/v1/entityOperations/upsert";
    // Each write's notification is read before the next write: a notification
    // still queued on the socket takes the next one of the same Entity into it
    // (coalescing, `x-coalesced`), so two writes in a row may arrive as one.
    let mut socket = socket;
    let mut messages = Vec::new();
    for speed in [10, 20] {
        let batch = json!([{"id": "urn:ngsi-ld:Vehicle:hel.fi:helsinki:1", "type": "Vehicle",
                            "speed": {"type": "Property", "value": speed}}]);
        let (status, _, body) = send(&app, "POST", uri, &writer, Some(&batch)).await;
        assert!(status.is_success(), "upsert {speed}: {status} {body}");
        let (back, got) = tokio::task::spawn_blocking(move || {
            let got = socket.next();
            (socket, got)
        })
        .await
        .expect("read");
        socket = back;
        messages.extend(got);
    }
    let rest = tokio::task::spawn_blocking(move || {
        std::iter::from_fn(|| socket.next()).collect::<Vec<Value>>()
    })
    .await
    .expect("read");
    messages.extend(rest);
    let speeds: Vec<&Value> = messages
        .iter()
        .filter(|m| m["metadata"]["x-op"] == "x-notification")
        .flat_map(|m| m["body"]["data"].as_array().into_iter().flatten())
        .map(|e| &e["speed"]["value"])
        .collect();
    assert_eq!(speeds, [&json!(10), &json!(20)], "{messages:?}");
}

/// T-4133, ADR-N-049: the platform gateway's fan-out subscribes on a view whose own Policy
/// grants only the role `public`, under its account and with no role, exactly as dev sends it.
/// A listed view is decided by its listed policies alone: without the `@context-gateway` listing
/// the subscription is refused 403 (dev, apply 67), with it the subscription is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_subscribes_on_a_public_view_only_where_it_is_listed() {
    let (app, _reload) = broker().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("an address").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    let tenant = "hel";
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema");
    let view = json!({"id": "transport", "tenant": tenant, "operations": ops,
        "selector": ["Vehicle"], "rules": [{"types": ["Vehicle"], "attributes": ["speed"]}]});
    for (uri, doc) in [
        ("/q/views/transport".to_owned(), view),
        (format!("/q/schemas/{tenant}"), schema),
    ] {
        let (status, _, body) = send(&app, "PUT", &uri, &[], Some(&doc)).await;
        assert!(status.is_success(), "{uri}: {status} {body}");
    }
    for (listed, expected) in [
        (json!(["public-transport"]), 403),
        (json!(["public-transport", "@context-gateway"]), 201),
    ] {
        let policy = json!({"policies": [
            {"id": "public-transport", "assignee": {"kind": "role", "id": "public"},
             "operations": ["5.7.1", "5.7.2"], "information": [{"entities": [{"type": "Vehicle"}]}]},
            {"id": "@context-gateway", "effect": "permit",
             "assignee": {"kind": "serviceAccount", "id": "context-gateway"},
             "operations": ["5.6.4", "5.6.5", "5.6.6", "5.7.1", "5.7.2", "5.7.5", "5.8.1", "5.8.3"]},
        ], "views": {"transport": listed}});
        let uri = format!("/q/policy/{tenant}");
        let (status, _, body) = send(&app, "PUT", &uri, &[], Some(&policy)).await;
        assert!(status.is_success(), "{uri}: {status} {body}");
        let made = tokio::task::spawn_blocking(move || {
            let mut socket = Socket::open(
                port,
                "/x/views/transport/ws",
                &[
                    ("x-jc-client".to_owned(), "context-gateway".to_owned()),
                    ("x-jc-roles".to_owned(), String::new()),
                ],
                Duration::from_millis(1500),
            )
            .expect("the socket");
            let _welcome = socket.next();
            let body = json!({"type": "Subscription", "entities": [{"type": "*"}],
                "notificationTrigger": ["entityCreated", "entityUpdated", "entityDeleted"],
                "notification": {"format": "normalized"}});
            socket
                .send(&json!({"metadata": {"x-op": "x-subscribe", "x-id": "up"}, "body": body}))
                .expect("sent");
            socket.next().unwrap_or_default()
        })
        .await
        .expect("ran");
        assert_eq!(
            made["metadata"]["x-status"], expected,
            "listed {listed}: {made}"
        );
    }
}
