//! An assistant's own MCP server, `/api/mcp/d/{publicId}` (T-4691, MF-52): exactly its
//! connectors' Endpoints and the reach of the named server it reads, `search_documents` only
//! when it has knowledge sources, read-only for every caller, and a public deployment never
//! serving an internal Endpoint. A publicId declared twice is nobody's. Every list is the one
//! resolver's (`jc_core::reach::resolve_connectors`, AG-121).

use crate::common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use context_gateway::app::{router, Gateway};
use context_gateway::auth::accounts::ServiceAccounts;
use context_gateway::mcp::documents::{hub_sources, Documents};
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::Endpoint;
use jc_core::kinds::{Audience, PolicySpec, Representation};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const HOST: &str = "https://city.example";
const AIR: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
const BIKES: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbb";
/// An organization Endpoint a public deployment names by mistake.
const STAFF: &str = "cccccccccccccccccccccccccc";
/// Public, and no connector of any deployment.
const OUTSIDE: &str = "dddddddddddddddddddddddddd";
/// Public and tagged TRAN, as is BIKES.
const PARKING: &str = "eeeeeeeeeeeeeeeeeeeeeeeeee";

/// The manifests: four Endpoints, a public deployment with two of them, the internal one and
/// a web source; one without sources; one by selector; an internal one; a disabled one.
fn repository() -> std::path::PathBuf {
    let endpoint = |name: &str, slug: &str, audience: &str| {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: {name}\n  \
             namespace: bbsk\nspec:\n  contextSpaceRef: data\n  slug: {slug}\n  \
             audience: {audience}\n  enabledRepresentations: [\"ngsi-ld\", \"mcp\"]\n"
        )
    };
    let transport =
        |name: &str, slug: &str| endpoint(name, slug, "public") + "  catalog: { themes: [TRAN] }\n";
    let deployment = |name: &str, extra: &str| {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: AssistantDeployment\nmetadata:\n  \
             name: {name}\n  namespace: bbsk\n  title: Dáta kraja\nspec:\n  publicId: bbsk-{name}\n\
             {extra}  allowedOrigins: [https://www.bbsk.sk]\n  \
             rateLimit: {{ requestsPerMinute: 60, perClientPerMinute: 10 }}\n  \
             budget: {{ tokensPerDay: 1000000, tokensPerConversation: 20000 }}\n"
        )
    };
    let connectors = "  connectors:\n    - { endpoint: air, tools: [query_entities] }\n    \
                      - { endpoint: bikes, tools: [query_entities] }\n    \
                      - { endpoint: staff, tools: [query_entities] }\n    \
                      - { endpoint: gone, tools: [query_entities] }\n";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gw-mcp-deployment-{now}"));
    std::fs::create_dir_all(&dir).expect("create the repository");
    for (file, body) in [
        (
            "space.yaml",
            "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: data\n  \
             namespace: bbsk\nspec:\n  isSandbox: false\n"
                .to_owned(),
        ),
        ("air.yaml", endpoint("air", AIR, "public")),
        ("bikes.yaml", transport("bikes", BIKES)),
        ("parking.yaml", transport("parking", PARKING)),
        ("staff.yaml", endpoint("staff", STAFF, "organization")),
        (
            "data.yaml",
            deployment(
                "data",
                &format!("  channel: public\n  sources: [web]\n{connectors}"),
            ),
        ),
        (
            "plain.yaml",
            deployment(
                "plain",
                "  channel: public\n  connectors:\n    - { endpoint: air, tools: [query_entities] }\n",
            ),
        ),
        (
            "transport.yaml",
            deployment(
                "transport",
                "  channel: public\n  select: { themes: [TRAN] }\n  exclude: [bikes]\n",
            ),
        ),
        (
            "internal.yaml",
            deployment(
                "internal",
                "  channel: internal\n  connectors:\n    - { endpoint: staff, tools: [query_entities] }\n",
            ),
        ),
        (
            "twice-a.yaml",
            deployment("twice-a", "  channel: public\n  sources: [web]\n")
                .replace("bbsk-twice-a", "bbsk-twice"),
        ),
        (
            "twice-b.yaml",
            deployment("twice-b", "  channel: public\n  sources: [web]\n")
                .replace("bbsk-twice-b", "bbsk-twice"),
        ),
        (
            "off.yaml",
            deployment(
                "off",
                "  channel: public\n  enabled: false\n  connectors:\n    - { endpoint: air, tools: [query_entities] }\n",
            ),
        ),
    ] {
        std::fs::write(dir.join(file), body).expect("write a manifest");
    }
    dir
}

#[test]
fn the_store_serves_each_enabled_deployment_with_its_connectors_and_sources() {
    let dir = repository();
    let servers = context_gateway::store::load(&dir)
        .expect("the repository loads")
        .6;
    std::fs::remove_dir_all(&dir).expect("clean up");
    let paths: Vec<String> = servers.iter().map(|s| s.path()).collect();
    assert_eq!(
        paths,
        [
            "/api/mcp/d/bbsk-data",
            "/api/mcp/d/bbsk-internal",
            "/api/mcp/d/bbsk-plain",
            "/api/mcp/d/bbsk-transport"
        ],
        "a disabled deployment serves no MCP, nor a publicId two deployments declare"
    );
    let data = &servers[0];
    assert_eq!(
        data.members,
        [AIR, BIKES],
        "a public deployment resolves no internal connector, and one naming no Endpoint is left out"
    );
    assert_eq!(data.audience, Audience::Public);
    assert_eq!(data.title.as_deref(), Some("Dáta kraja"));
    assert_eq!(
        data.knowledge_sources,
        [("bbsk".to_owned(), "web".to_owned())]
    );
    assert_eq!(servers[1].audience, Audience::Organization);
    assert_eq!(
        servers[1].members,
        [STAFF],
        "internal resolves its own project's every audience"
    );
    assert!(servers[2].knowledge_sources.is_empty());
    assert_eq!(
        servers[3].members,
        [PARKING],
        "the selector's TRAN Endpoints, the excluded one never"
    );
}

/// EP-122: a deployment reading `org/all` reads the Organization's every Endpoint without a
/// manifest for it; one naming an McpServer nobody declares reads no Endpoint through it; one
/// whose mcpServer is no `{project}/{name}` is not served; a theme title names a deployment
/// whose manifest has no title.
#[test]
fn a_deployment_reads_org_all_or_nothing_from_an_undeclared_server() {
    let deployment = |name: &str, server: &str| {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: AssistantDeployment\nmetadata:\n  \
             name: {name}\n  namespace: bbsk\nspec:\n  publicId: bbsk-{name}\n  channel: internal\n  \
             mcpServer: {server}\n  theme: {{ title: Kraj }}\n  allowedOrigins: [https://www.bbsk.sk]\n  \
             rateLimit: {{ requestsPerMinute: 60, perClientPerMinute: 10 }}\n  \
             budget: {{ tokensPerDay: 1000000, tokensPerConversation: 20000 }}\n"
        )
    };
    let dir = std::env::temp_dir().join(format!(
        "gw-mcp-deployment-org-all-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the repository");
    for (file, body) in [
        (
            "space.yaml",
            "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: data\n  \
             namespace: bbsk\nspec:\n  isSandbox: false\n"
                .to_owned(),
        ),
        (
            "air.yaml",
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air\n  \
                 namespace: bbsk\nspec:\n  contextSpaceRef: data\n  slug: {AIR}\n  \
                 audience: public\n  enabledRepresentations: [\"ngsi-ld\", \"mcp\"]\n"
            ),
        ),
        ("org.yaml", deployment("org", "org/all")),
        ("ghost.yaml", deployment("ghost", "bbsk/nobody")),
        ("bad.yaml", deployment("bad", "nonsense")),
    ] {
        std::fs::write(dir.join(file), body).expect("write a manifest");
    }
    let servers = context_gateway::store::load(&dir)
        .expect("the repository loads")
        .6;
    std::fs::remove_dir_all(&dir).expect("clean up");
    let named = |path: &str| servers.iter().find(|s| s.path() == path);
    assert!(
        named("/api/mcp/d/bbsk-bad").is_none(),
        "an invalid mcpServer"
    );
    let org = named("/api/mcp/d/bbsk-org").expect("the org/all deployment");
    assert_eq!(org.title.as_deref(), Some("Kraj"));
    assert!(named("/api/mcp/d/bbsk-ghost").is_none_or(|ghost| ghost.members.is_empty()));
}

/// A deployment that resolves more Endpoints than it may read serves the first
/// `MAX_RESOLVED` by slug and leaves the rest out.
#[test]
fn a_deployment_past_its_endpoint_bound_serves_the_first_by_slug() {
    let bound = jc_core::reach::MAX_RESOLVED;
    let slug = |n: usize| {
        let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
        let tail: String = [15, 10, 5, 0]
            .iter()
            .map(|shift| char::from(alphabet[(n >> shift) & 31]))
            .collect();
        format!("{}{tail}", "a".repeat(22))
    };
    let dir = std::env::temp_dir().join(format!(
        "gw-mcp-deployment-bound-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the repository");
    let write = |file: String, body: String| std::fs::write(dir.join(file), body).expect("write");
    write(
        "space.yaml".into(),
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: data\n  \
         namespace: bbsk\nspec:\n  isSandbox: false\n"
            .to_owned(),
    );
    for n in 0..=bound {
        write(
            format!("e{n}.yaml"),
            format!(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: e{n}\n  \
                 namespace: bbsk\nspec:\n  contextSpaceRef: data\n  slug: {}\n  \
                 audience: public\n  enabledRepresentations: [\"ngsi-ld\", \"mcp\"]\n",
                slug(n)
            ),
        );
    }
    write(
        "wide.yaml".into(),
        "apiVersion: joinedcontext.com/v1alpha1\nkind: AssistantDeployment\nmetadata:\n  \
         name: wide\n  namespace: bbsk\nspec:\n  publicId: bbsk-wide\n  channel: internal\n  \
         mcpServer: org/all\n  allowedOrigins: [https://www.bbsk.sk]\n  \
         rateLimit: { requestsPerMinute: 60, perClientPerMinute: 10 }\n  \
         budget: { tokensPerDay: 1000000, tokensPerConversation: 20000 }\n"
            .to_owned(),
    );
    let (endpoints, _, _, _, _, _, servers, ..) =
        context_gateway::store::load(&dir).expect("the repository loads");
    std::fs::remove_dir_all(&dir).expect("clean up");
    assert_eq!(endpoints.len(), bound + 1, "every Endpoint loads");
    let wide = servers
        .iter()
        .find(|s| s.path() == "/api/mcp/d/bbsk-wide")
        .expect("the deployment");
    // By slug is by byte: the base32 digits sort before the letters.
    let mut by_slug: Vec<String> = (0..=bound).map(slug).collect();
    by_slug.sort();
    assert_eq!(wide.members.len(), bound);
    assert_eq!(wide.members.first(), by_slug.first());
    assert!(
        !wide.members.contains(&by_slug[bound]),
        "the last by slug is left out"
    );
}

/// A stand-in `jc-assistant` answering the sources summary, one search and `bbsk-data`'s context.
async fn assistant() -> String {
    let stub = Router::new().fallback(|request: axum::extract::Request| async move {
        axum::Json(if request.uri().path() == "/internal/v1/sources" {
            json!({"passages": 3, "publishers": ["bbsk"], "languages": {"sk": 1.0}})
        } else if request.uri().path() == "/api/v1/d/bbsk-data/context" {
            json!({"publicId": "bbsk-data", "endpoints": [{"slug": AIR}], "sources": [], "omitted": 0})
        } else {
            json!({"hits": []})
        })
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, stub).await;
    });
    url
}

fn endpoint(slug: &str, audience: Audience, role: &str) -> Endpoint {
    let policy: PolicySpec = serde_norway::from_str(&format!(
        "contextSpaceRef: data\nassigner: did:web:bbsk.sk\nassignee: {{ kind: role, id: {role} }}\n\
         operations: [queryEntity, upsertBatch]\ninformation:\n  - entities:\n      - type: Event\n"
    ))
    .expect("the policy spec parses");
    Endpoint {
        declared_types: None,
        roles: Default::default(),
        slug: slug.to_owned(),
        title: Default::default(),
        description: Default::default(),
        space: "data".to_owned(),
        tenant: context_gateway::middleware::tenancy::Tenant::segment("data").ok(),
        project: "bbsk".to_owned(),
        audience,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd, Representation::Mcp],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: Default::default(),
        projection: None,
        view_mapping: None,
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        open_data: false,
        base_path: format!("/api/endpoint/{slug}"),
        models: Vec::new(),
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![policy],
    }
}

/// The gateway with the deployments the store loads from [`repository`].
async fn app(realm: &common::Realm) -> Router {
    app_reading(realm, &assistant().await).await
}

/// [`app`] with the assistant at `assistant_url`.
async fn app_reading(realm: &common::Realm, assistant_url: &str) -> Router {
    let dir = repository();
    let servers = context_gateway::store::load(&dir)
        .expect("the repository loads")
        .6;
    std::fs::remove_dir_all(&dir).expect("clean up");
    let gateway = Gateway::new(
        Broker::new("http://127.0.0.1:9"),
        Box::new(PolicyPdp),
        "bbsk.sk",
    )
    .serve(vec![
        endpoint(AIR, Audience::Public, "public"),
        endpoint(BIKES, Audience::Public, "public"),
        endpoint(STAFF, Audience::Organization, "steward"),
        endpoint(OUTSIDE, Audience::Public, "public"),
    ])
    .authenticate(
        Arc::new(realm.verifier()),
        ServiceAccounts::new(),
        Some(HOST.to_owned()),
    )
    .search_documents_with(Documents::new(assistant_url, hub_sources("bbsk/web")));
    gateway.replace_servers(servers);
    router(Arc::new(gateway))
}

/// A steward of `bbsk` with a token for the edge.
fn steward(realm: &common::Realm) -> String {
    realm.mint(&json!({
        "iss": common::ISSUER,
        "sub": "0f5a",
        "aud": "context-gateway",
        "scope": "openid",
        "preferred_username": "jana",
        "groups": ["/bbsk"],
        "realm_access": { "roles": ["steward"] },
        "exp": common::in_seconds(300),
        "iat": common::in_seconds(-10),
    }))
}

async fn rpc(app: &Router, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-forwarded-for", "203.0.113.7");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .body(Body::from(body.to_string()))
        .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("a body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn list() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
}

fn call(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}})
}

fn names(answer: &Value) -> Vec<String> {
    answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tool list: {answer}"))
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The text of a tool answer, the JSON an MCP client reads.
fn text(answer: &Value) -> String {
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no tool text: {answer}"))
        .to_owned()
}

#[tokio::test]
async fn a_public_deployment_lists_its_public_endpoints_and_search_documents_only() {
    let realm = common::Realm::new();
    let app = app(&realm).await;
    let (status, tools) = rpc(&app, "/api/mcp/d/bbsk-data", None, list()).await;
    assert_eq!(status, StatusCode::OK, "{tools}");
    let tools = names(&tools);
    assert!(tools.contains(&"search_documents".to_owned()), "{tools:?}");
    let (_, plain) = rpc(&app, "/api/mcp/d/bbsk-plain", None, list()).await;
    assert!(
        !names(&plain).contains(&"search_documents".to_owned()),
        "a deployment without sources has no search_documents: {plain}"
    );

    let (_, listed) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        call("list_endpoints", json!({})),
    )
    .await;
    let listed = text(&listed);
    assert!(listed.contains(AIR) && listed.contains(BIKES), "{listed}");
    assert!(
        !listed.contains(STAFF) && !listed.contains(OUTSIDE),
        "neither an internal connector nor an unnamed Endpoint: {listed}"
    );
}

#[tokio::test]
async fn a_signed_in_caller_gets_no_write_tool_and_no_internal_endpoint() {
    let realm = common::Realm::new();
    let app = app(&realm).await;
    let token = steward(&realm);
    let (status, tools) = rpc(&app, "/api/mcp/d/bbsk-data", Some(&token), list()).await;
    assert_eq!(status, StatusCode::OK, "{tools}");
    let (_, anonymous) = rpc(&app, "/api/mcp/d/bbsk-data", None, list()).await;
    assert_eq!(
        names(&tools),
        names(&anonymous),
        "a token opens no write tool on an assistant's server"
    );
    let (_, listed) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        Some(&token),
        call("list_endpoints", json!({})),
    )
    .await;
    assert!(!text(&listed).contains(STAFF), "{listed}");
}

#[tokio::test]
async fn an_endpoint_outside_the_deployment_answers_as_an_unknown_one() {
    let realm = common::Realm::new();
    let app = app(&realm).await;
    let read = |slug: &str| {
        call(
            "query_entities",
            json!({ "endpoint": slug, "type": ["Event"] }),
        )
    };
    let (_, unknown) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        read("zzzzzzzzzzzzzzzzzzzzzzzzzy"),
    )
    .await;
    for outside in [OUTSIDE, STAFF] {
        let (_, answer) = rpc(&app, "/api/mcp/d/bbsk-data", None, read(outside)).await;
        assert_eq!(
            answer
                .to_string()
                .replace(outside, "zzzzzzzzzzzzzzzzzzzzzzzzzy"),
            unknown.to_string(),
            "{outside}"
        );
    }
}

#[tokio::test]
async fn an_internal_deployment_asks_for_a_token_and_an_unknown_one_is_404() {
    let realm = common::Realm::new();
    let app = app(&realm).await;
    let (status, _) = rpc(&app, "/api/mcp/d/bbsk-internal", None, list()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = rpc(&app, "/api/mcp/d/bbsk-off", None, list()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a disabled deployment");
    let (status, _) = rpc(&app, "/api/mcp/d/nobody", None, list()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// An assistant that cannot be read leaves the `context` resource a JSON-RPC internal error that
/// says to try again, and nothing of the assistant's address or its error.
#[tokio::test]
async fn an_assistant_that_cannot_be_read_leaves_its_context_an_internal_error() {
    let realm = common::Realm::new();
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        format!("http://{}", listener.local_addr().expect("an address"))
    };
    let app = app_reading(&realm, &closed).await;
    let (_, answer) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        json!({"jsonrpc": "2.0", "id": 2, "method": "resources/read",
               "params": {"uri": "context://bbsk-data"}}),
    )
    .await;
    assert_eq!(answer["error"]["code"], -32603, "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("try again shortly") && !m.contains("127.0.0.1")),
        "{answer}"
    );
}

/// EP-136, API/05 §5: a public assistant's server offers the resource `context`, the
/// assistant's own document; an internal one and any other URI offer nothing.
#[tokio::test]
async fn a_public_assistant_offers_its_context_as_a_resource() {
    let realm = common::Realm::new();
    let app = app(&realm).await;
    let (_, initialized) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}}),
    )
    .await;
    assert!(
        initialized["result"]["capabilities"]["resources"].is_object(),
        "{initialized}"
    );
    let (_, listed) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
    )
    .await;
    assert_eq!(
        listed["result"]["resources"][0]["uri"], "context://bbsk-data",
        "{listed}"
    );
    let read = |uri: &str| json!({"jsonrpc": "2.0", "id": 2, "method": "resources/read", "params": {"uri": uri}});
    let (_, answer) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        read("context://bbsk-data"),
    )
    .await;
    let text = answer["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no contents: {answer}"));
    let context: Value = serde_json::from_str(text).expect("the document");
    assert_eq!(context["endpoints"][0]["slug"], AIR);
    let (_, other) = rpc(
        &app,
        "/api/mcp/d/bbsk-data",
        None,
        read("context://bbsk-plain"),
    )
    .await;
    assert_eq!(
        other["error"]["code"], -32002,
        "another assistant's context: {other}"
    );

    let token = steward(&realm);
    let (_, internal) = rpc(
        &app,
        "/api/mcp/d/bbsk-internal",
        Some(&token),
        json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
    )
    .await;
    // No context of its own: only the chart view every server lists (T-6882).
    assert_eq!(
        internal["result"]["resources"]
            .as_array()
            .map(|all| all.iter().map(|r| r["uri"].clone()).collect::<Vec<_>>()),
        Some(vec![json!("ui://joinedcontext/chart-view.html")]),
        "{internal}"
    );
}
