//! The authorization matrix (T-2797, PF-52, AC-01): every route the Portal serves, called as each
//! of the roles of the taxonomy, answers what `tests/authz_matrix.yaml` says, on every push.
//!
//! The roles are the demo ones (deployment's `helsinki-role-*.yaml`): a viewer reads; an editor
//! proposes and approves the modelling kinds; a steward proposes and approves every project kind;
//! an approver only approves; the org-admin proposes, approves and deletes everything and
//! manages people; a member of another project is a steward there and nothing here; a project
//! ServiceAccount reads through its own bearer token. Everyone also reads, as every person
//! does through `platform-readers`, except the member of the other project.
//!
//! A cell is `allow` or the status the caller gets instead. `allow` is any answer that is not a
//! refusal of the caller: never 401 or 403, and not 404 either on a row where somebody else's
//! refusal is 404, so a route that hides a project cannot pass by losing the object. A 503 in a
//! cell is a route whose backing service (the forge, the realm's admin API, the database) the
//! fixture does not run, reached before the caller is asked about; a 422 is a body refused
//! before the caller is asked about. The row's note says which. The nightly run on dev reads the
//! same table (joinedcontext-conformance `tests/authz`).
//!
//! `JC_AUTHZ_RECORD=<file>` writes what every cell answered instead of asserting, for a new row.

mod common;

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::{cookie, envelope, person, CSRF};
use joinedcontext_portal::auth::csrf::CSRF_HEADER;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const TABLE: &str = include_str!("authz_matrix.yaml");
const PROJECT: &str = "helsinki";
const OTHER_PROJECT: &str = "praha";
/// The name every seeded object of the fixture carries.
const EXISTING: &str = "existing";
const SERVICE_ACCOUNT_CLIENT: &str = "helsinki-reader";

const ROLES: [&str; 8] = [
    "anonymous",
    "viewer",
    "editor",
    "steward",
    "approver",
    "org-admin",
    "other-member",
    "service-account",
];

/// What every person reads (deployment's helsinki-role-viewer.yaml, verbatim).
const READ_KINDS: [&str; 32] = [
    "Organization",
    "Project",
    "ContextSpace",
    "DataModel",
    "Mapping",
    "Policy",
    "ScopeDefinition",
    "Endpoint",
    "ModelProjection",
    "SharedSpaceReference",
    "ContextSourceRegistration",
    "ServiceAccount",
    "Pipeline",
    "DataSource",
    "MqttBroker",
    "App",
    "CkanInstance",
    "KnowledgeSource",
    "AssistantDeployment",
    "McpServer",
    "Blueprint",
    "AgentProfile",
    "DataSpaceParticipant",
    "DataOffer",
    "DataAgreement",
    "SyncSource",
    "Bundle",
    "UiSchema",
    "Subscription",
    "Entity",
    "Dashboard",
    "Layer",
];
/// What the steward proposes and approves and the approver approves (helsinki-role-steward.yaml):
/// every project kind but `CkanInstance`, which the administrator hands out per instance (PF-107).
const STEWARD_KINDS: [&str; 25] = [
    "ContextSpace",
    "DataModel",
    "Mapping",
    "Policy",
    "ScopeDefinition",
    "Endpoint",
    "ModelProjection",
    "SharedSpaceReference",
    "ContextSourceRegistration",
    "ServiceAccount",
    "Pipeline",
    "DataSource",
    "MqttBroker",
    "App",
    "Blueprint",
    "DataSpaceParticipant",
    "DataOffer",
    "DataAgreement",
    "SyncSource",
    "Bundle",
    "UiSchema",
    "Subscription",
    "Entity",
    "Dashboard",
    "Layer",
];
/// What the editor proposes and approves (helsinki-role-editor.yaml).
const EDITOR_KINDS: [&str; 8] = [
    "ContextSpace",
    "DataModel",
    "Mapping",
    "Policy",
    "Endpoint",
    "ModelProjection",
    "Pipeline",
    "DataSource",
];
/// What the org-admin proposes, approves and deletes (helsinki-role-org-admin.yaml).
const ADMIN_KINDS: [&str; 34] = [
    "Organization",
    "Project",
    "ContextSpace",
    "DataModel",
    "Mapping",
    "Policy",
    "ScopeDefinition",
    "Endpoint",
    "ModelProjection",
    "SharedSpaceReference",
    "ContextSourceRegistration",
    "ServiceAccount",
    "Pipeline",
    "DataSource",
    "MqttBroker",
    "App",
    "CkanInstance",
    "KnowledgeSource",
    "AssistantDeployment",
    "McpServer",
    "Blueprint",
    "DataSpaceParticipant",
    "DataOffer",
    "DataAgreement",
    "SyncSource",
    "Bundle",
    "UiSchema",
    "Role",
    "RoleBinding",
    "Group",
    "Subscription",
    "Entity",
    "Dashboard",
    "Layer",
];

/// What a cell may say: allowed, or the status the caller gets instead.
const CELLS: [&str; 9] = [
    "allow", "400", "401", "403", "404", "405", "415", "422", "503",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    roles: Vec<String>,
    routes: Vec<Row>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct Row {
    /// `METHOD /path` as the route inventory names it, `/api/v1` left off.
    route: String,
    /// Who may call it, in the words of the classification (public, signed-in, project-read,
    /// project-verb:<verb>:<kind>, people-admin, workload:<client>, signature:<which>, app-edge,
    /// run-owner, mcp-dispatch, org-setting:<field>).
    rule: String,
    /// One cell per role, in the order of `roles`.
    expect: Vec<String>,
    /// The body a write is sent with; `{}` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    /// A query string the route needs before it asks about the caller (`q=air`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    /// Placeholder values of this row where the fixture's defaults name nothing (`name: jc_project_get`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<BTreeMap<String, String>>,
    /// A form body instead (`application/x-www-form-urlencoded`), for the back-channel logout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    form: Option<String>,
    /// Whether the nightly run on dev may send it. A read always may; a write only when the row
    /// says so: a proposal sent with `?dryRun=All`, or a refusal on an object that does not
    /// exist, so a check that came loose on dev still writes nothing there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    live: Option<bool>,
    /// Why a cell reads as it does, where the rule alone does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

fn table() -> Table {
    serde_norway::from_str(TABLE).expect("tests/authz_matrix.yaml parses")
}

fn split(route: &str) -> (&str, &str) {
    route
        .split_once(' ')
        .expect("a row's route is `METHOD /path`")
}

#[test]
fn every_route_has_a_row_and_every_row_a_route() {
    let table = table();
    assert_eq!(
        table.roles, ROLES,
        "the table's roles are the taxonomy's, in this order"
    );
    let rows: Vec<(String, String)> = table
        .routes
        .iter()
        .map(|row| {
            let (method, path) = split(&row.route);
            (method.to_owned(), path.to_owned())
        })
        .collect();
    let served = common::routes::routes_in_source();
    let missing: Vec<String> = served
        .iter()
        .filter(|route| !rows.contains(route))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    let stale: Vec<String> = rows
        .iter()
        .filter(|route| !served.contains(route))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        missing.is_empty(),
        "a route without a row in tests/authz_matrix.yaml: say who may call it\n  {}",
        missing.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "a row for a route that is gone:\n  {}",
        stale.join("\n  ")
    );
    let mut seen = std::collections::BTreeSet::new();
    for row in &table.routes {
        assert!(seen.insert(&row.route), "{} has two rows", row.route);
        assert_eq!(
            row.expect.len(),
            ROLES.len(),
            "{}: one cell per role",
            row.route
        );
        let (method, _) = split(&row.route);
        assert!(
            !(row.live == Some(true)
                && method != "GET"
                && !row.route.contains("{plural}")
                && row.note.is_none()),
            "{}: a write the nightly run sends on dev says why it cannot write there (note)",
            row.route
        );
        for cell in &row.expect {
            assert!(
                CELLS.contains(&cell.as_str()),
                "{}: `{cell}` is not one of {CELLS:?}",
                row.route
            );
        }
    }
}

/// Where a route is served: the internal listener, the root, or the API.
fn url(path: &str, row: &Row) -> (bool, String) {
    let mut filled = path.to_owned();
    for (placeholder, value) in row.values.iter().flatten() {
        filled = filled.replace(&format!("{{{placeholder}}}"), value);
    }
    let mut filled = fill(&filled);
    if let Some(query) = &row.query {
        filled = format!("{filled}?{query}");
    }
    if filled.starts_with("/internal/") {
        (true, filled)
    } else if ["/apps/", "/metrics", "/.well-known/", "/c/"]
        .iter()
        .any(|root| filled.starts_with(root))
    {
        (false, filled)
    } else {
        (false, format!("/api/v1{filled}"))
    }
}

/// A route's placeholders, filled with what the fixture holds.
fn fill(path: &str) -> String {
    let values: [(&str, &str); 25] = [
        ("{project}", PROJECT),
        ("{plural}", "endpoints"),
        ("{name}", EXISTING),
        ("{id}", "0000000000000000"),
        ("{kind}", "Endpoint"),
        ("{space}", PROJECT),
        ("{asset}", "logo"),
        ("{fn}", "summary"),
        ("{style}", "streets"),
        ("{z}", "1"),
        ("{x}", "1"),
        ("{tile}", "1.png"),
        ("{keyId}", "k1"),
        ("{*path}", "index.js"),
        ("{component}", "proxy"),
        ("{run}", "r1"),
        ("{version}", "v1"),
        ("{artifact}", "schema.json"),
        ("{style_id}", "streets"),
        ("{slug}", "s1"),
        ("{source}", EXISTING),
        ("{page}", "1"),
        ("{deployment}", EXISTING),
        ("{type}", "Station"),
        ("{turn}", "1"),
    ];
    values
        .iter()
        .fold(path.to_owned(), |out, (from, to)| out.replace(from, to))
}

/// A Portal whose bearer verifier trusts the test realm, with the roles of the taxonomy bound.
async fn fixture() -> AppState {
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_AGENT_PROXY_CLIENT_ID" => Some(common::AGENT_PROXY_CLIENT),
            "JC_PORTAL_PIPELINE_RUNNER_CLIENT_ID" => Some(common::PIPELINE_RUNNER_CLIENT),
            "JC_PORTAL_GATEWAY_CLIENT_ID" => Some("context-gateway"),
            "JC_GITEA_WEBHOOK_SECRET" => Some("webhook-secret-of-the-fixture"),
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    // A forge that takes every write, so a route gets past "no forge" to the caller's check.
    let forge = common::forge().await;
    let client = joinedcontext_portal::git::GiteaClient::new(
        forge.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(config, None).with_gitea(std::sync::Arc::new(client));
    // The mock forge answers for as long as the test binary runs.
    std::mem::forget(forge);
    if let Some(bearer) = &state.bearer {
        bearer.refresh().await.expect("the test realm's keys");
    }
    // The space's model as the reconciler compiles it, so a type's attributes are there to read
    // (T-3223).
    state.model_schemas.replace(std::collections::HashMap::from([(
        (PROJECT.to_owned(), PROJECT.to_owned()),
        std::sync::Arc::new(joinedcontext_portal::pipeline_validation::ModelSchema::compile(
            "stations",
            "1.0.0",
            &json!({ "definitions": { "Station": { "properties": { "name": { "type": "string" } } } } }),
            &["Station".to_owned()],
            false,
        )),
    )]));
    let bind = |name: &str, rules: Value, subject: Value, scope: Value| {
        state.mirror.upsert(envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": rules }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{name}-binding"),
            ORG_NAMESPACE,
            json!({ "subjects": [subject], "role": name, "scope": scope }),
        ));
    };
    let here = json!({ "project": PROJECT });
    let read = json!({ "kinds": READ_KINDS.as_slice(), "verbs": ["read"] });
    let user = |who: &str| json!({ "user": format!("{who}@hel.fi") });
    bind("viewer", json!([read]), user("viewer"), here.clone());
    bind(
        "editor",
        json!([
            read,
            { "kinds": EDITOR_KINDS, "verbs": ["propose", "approve"] },
            { "kinds": ["RoleBinding"], "verbs": ["propose"] }
        ]),
        user("editor"),
        here.clone(),
    );
    bind(
        "steward",
        json!([read, { "kinds": STEWARD_KINDS, "verbs": ["propose", "approve"] }]),
        user("steward"),
        here.clone(),
    );
    bind(
        "approver",
        json!([read, { "kinds": STEWARD_KINDS, "verbs": ["approve"] }]),
        user("approver"),
        here.clone(),
    );
    bind(
        "org-admin",
        json!([
            read,
            { "kinds": ADMIN_KINDS.as_slice(), "verbs": ["propose", "approve", "delete"] },
            { "kinds": ["Person"], "verbs": ["read", "create", "update", "disable", "delete"] }
        ]),
        user("org-admin"),
        json!({ "organization": "hel" }),
    );
    bind(
        "other-member",
        json!([read, { "kinds": STEWARD_KINDS, "verbs": ["propose", "approve"] }]),
        user("other-member"),
        json!({ "project": OTHER_PROJECT }),
    );
    bind(
        "service-account",
        json!([read]),
        json!({ "user": format!("service-account-{SERVICE_ACCOUNT_CLIENT}") }),
        here,
    );
    for kind in [
        "Endpoint",
        "Pipeline",
        "DataModel",
        "SyncSource",
        "ServiceAccount",
        "App",
        "CkanInstance",
    ] {
        state
            .mirror
            .upsert(envelope(kind, EXISTING, PROJECT, json!({})));
    }
    // A saved source the connection test reads (T-4819); the fixture runs no runner, so it is 503.
    state.mirror.upsert(envelope(
        "DataSource",
        EXISTING,
        PROJECT,
        json!({ "type": "http", "http": { "url": "https://feeds.example/existing.json" } }),
    ));
    state.mirror.upsert(envelope(
        "MqttBroker",
        EXISTING,
        PROJECT,
        json!({ "roles": [{ "name": "camera", "serviceAccounts": [EXISTING], "publish": ["cameras/#"] }] }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        PROJECT,
        PROJECT,
        json!({ "isSandbox": false }),
    ));
    for project in [PROJECT, OTHER_PROJECT] {
        state.mirror.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            // MQTT on, so a device proposal meets the permission rule and not the allowance (PF-117).
            json!({ "organizationRef": "hel", "repository": { "name": project }, "ref": "main",
                    "quotas": { "mqtt": { "enabled": true, "brokers": 2, "connections": 2000 } } }),
        ));
    }
    state
}

/// One call as `role`: a session cookie for a person who proved a second factor (the table
/// measures roles; the factor has its own test below), the realm's token for the
/// ServiceAccount, nothing for anonymous.
async fn call(state: &AppState, role: &str, method: &str, path: &str, row: &Row) -> StatusCode {
    send(state, role, true, method, path, row).await.status()
}

async fn send(
    state: &AppState,
    role: &str,
    factor: bool,
    method: &str,
    path: &str,
    row: &Row,
) -> axum::response::Response {
    let (internal, uri) = url(path, row);
    let content_type = match (&row.form, method) {
        (Some(_), _) => "application/x-www-form-urlencoded",
        (None, "PATCH") => "application/merge-patch+json",
        (None, _) => "application/json",
    };
    let mut request = Request::builder()
        .method(method)
        .uri(&uri)
        .header(header::CONTENT_TYPE, content_type);
    match role {
        "anonymous" => {}
        "service-account" => {
            let token = common::REALM.token(SERVICE_ACCOUNT_CLIENT, "portal-api");
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        who => {
            let identity = Identity {
                second_factor: factor,
                ..person(who)
            };
            request = request
                .header(header::COOKIE, cookie(&state.config, identity))
                .header(CSRF_HEADER, CSRF);
        }
    }
    let body = match (&row.form, &row.body) {
        _ if !["POST", "PUT", "PATCH"].contains(&method) => Body::empty(),
        (Some(form), _) => Body::from(form.clone()),
        (None, Some(body)) => Body::from(body.to_string()),
        (None, None) => Body::from("{}"),
    };
    let router = if internal {
        server::internal_app(state.clone())
    } else {
        server::app(state.clone())
    };
    router
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response")
}

fn cell(status: StatusCode) -> String {
    if status.is_success() || status.is_redirection() {
        "allow".to_owned()
    } else {
        status.as_u16().to_string()
    }
}

fn holds(expect: &str, status: StatusCode, hides: bool) -> bool {
    match expect {
        "allow" => {
            !matches!(status.as_u16(), 401 | 403) && !(hides && status == StatusCode::NOT_FOUND)
        }
        refusal => status.as_u16().to_string() == refusal,
    }
}

#[tokio::test]
async fn every_role_meets_every_route_as_the_table_says() {
    let table = table();
    let state = fixture().await;
    let record = std::env::var("JC_AUTHZ_RECORD").ok();
    let mut misses = Vec::new();
    let mut recorded: Vec<Row> = Vec::new();
    // Signing out ends every session of that person issued up to that second, so the two
    // sign-out routes run after every other row.
    let mut rows: Vec<&Row> = table.routes.iter().collect();
    rows.sort_by_key(|row| row.route.contains("logout"));
    for row in rows {
        let (method, path) = split(&row.route);
        let hides = row.expect.iter().any(|cell| cell == "404");
        let mut seen = Vec::new();
        for (role, expect) in ROLES.iter().zip(&row.expect) {
            let status = call(&state, role, method, path, row).await;
            seen.push(cell(status));
            if !holds(expect, status, hides) {
                misses.push(format!(
                    "{} as {role}: expected {expect}, answered {status}",
                    row.route
                ));
            }
        }
        recorded.push(Row {
            expect: seen,
            ..row.clone()
        });
    }
    if let Some(file) = record {
        let mut out = BTreeMap::new();
        out.insert("routes", recorded);
        std::fs::write(&file, serde_norway::to_string(&out).expect("yaml"))
            .expect("write the record");
        return;
    }
    assert!(
        misses.is_empty(),
        "{} cells of tests/authz_matrix.yaml do not hold:\n  {}",
        misses.len(),
        misses.join("\n  ")
    );
}

/// T-5567 (AUT-2, PF-128): the decision doors, and only they, refuse a person who holds the right
/// but proved no second factor in the session, with `second-factor-required`. Every cell the
/// table allows a person is called again without the factor; the routes that change answer form
/// the list below, so a door that loses its gate, or a new one without it, turns this red. The
/// access-request doors answer 503 here (no database) before the caller is asked about;
/// `access_requests_tests` covers them.
#[tokio::test]
async fn a_decision_without_a_second_factor_is_refused_at_every_door() {
    let table = table();
    let state = fixture().await;
    let mut gated = std::collections::BTreeSet::new();
    let mut misses = Vec::new();
    let mut rows: Vec<&Row> = table.routes.iter().collect();
    rows.sort_by_key(|row| row.route.contains("logout"));
    for row in rows {
        let (method, path) = split(&row.route);
        let hides = row.expect.iter().any(|cell| cell == "404");
        for (role, expect) in ROLES.iter().zip(&row.expect) {
            if expect != "allow" || ["anonymous", "service-account"].contains(role) {
                continue;
            }
            let answer = send(&state, role, false, method, path, row).await;
            if holds(expect, answer.status(), hides) {
                continue;
            }
            let status = answer.status();
            let body = axum::body::to_bytes(answer.into_body(), 1 << 20)
                .await
                .expect("body");
            let problem: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let slug = problem["type"].as_str().unwrap_or_default();
            if status == StatusCode::FORBIDDEN && slug.ends_with("second-factor-required") {
                gated.insert(row.route.clone());
            } else {
                misses.push(format!("{} as {role}: answered {status} {slug}", row.route));
            }
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
    let expected = [
        "DELETE /organization/people/{id}",
        "DELETE /organization/people/{id}/consents/{client}",
        "PATCH /organization/people/{id}",
        "POST /organization/people",
        "POST /organization/people/name-by-id",
        "POST /organization/people/remove",
        "POST /organization/people/{id}/disable",
        "POST /organization/people/{id}/enable",
        "POST /organization/people/{id}/remove-second-factor",
        "POST /organization/people/{id}/resend-invitation",
        "POST /organization/people/{id}/reset-password",
        "POST /organization/people/{id}/sign-out",
        "POST /projects/{project}/changes/{id}/approve",
        "POST /projects/{project}/changes/{id}/reject",
        "POST /projects/{project}/serviceaccounts/{name}/revoke",
        "POST /projects/{project}/serviceaccounts/{name}/secret",
        "POST /projects/{project}/spaces/{space}/erasures/{id}/approve",
        "POST /projects/{project}/spaces/{space}/erasures/{id}/decline",
    ];
    let gated: Vec<&str> = gated.iter().map(String::as_str).collect();
    assert_eq!(gated, expected, "the doors that want a second factor");
}

/// PF-107 (T-3046): a `CkanInstance` is changed by the organization administrator and by whoever
/// holds that instance's own role, on that instance alone; the steward, the approver and the
/// editor, whose roles name every other project kind, are refused, and so is the holder of one
/// instance's role on another instance.
#[tokio::test]
async fn a_ckan_instance_is_changed_by_the_administrator_and_its_own_role_alone() {
    let state = fixture().await;
    let own = format!("ckan-admin-{PROJECT}-{EXISTING}");
    state
        .mirror
        .upsert(envelope("CkanInstance", "second", PROJECT, json!({})));
    state.mirror.upsert(envelope(
        "Role",
        &own,
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["CkanInstance"],
            "verbs": ["propose", "approve", "delete"],
            "constraints": [{ "field": "metadata.name", "in": [EXISTING] }],
        }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        &own,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "ckan-admin@hel.fi" }], "role": own, "scope": { "project": PROJECT } }),
    ));
    let manifest = |name: &str| {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "CkanInstance",
            "metadata": { "name": name, "namespace": PROJECT },
            "spec": { "url": "https://data.example.org", "apiTokenRef": { "name": "ckan-token", "key": "apiToken" } },
        })
    };
    let mut misses = Vec::new();
    for (instance, who, expect) in [
        (EXISTING, "org-admin", "allow"),
        (EXISTING, "ckan-admin", "allow"),
        ("second", "org-admin", "allow"),
        ("second", "ckan-admin", "403"),
        (EXISTING, "steward", "403"),
        (EXISTING, "approver", "403"),
        (EXISTING, "editor", "403"),
        (EXISTING, "viewer", "403"),
    ] {
        for (method, query) in [
            ("PUT", "dryRun=All".to_owned()),
            ("DELETE", format!("confirm={instance}")),
        ] {
            let row: Row = serde_json::from_value(json!({
                "route": format!("{method} /projects/{{project}}/ckaninstances/{{name}}"),
                "rule": "project-verb:propose:CkanInstance",
                "expect": [],
                "body": manifest(instance),
                "query": query,
                "values": { "name": instance },
            }))
            .expect("a row");
            let (_, path) = split(&row.route);
            let status = call(&state, who, method, path, &row).await;
            if !holds(expect, status, false) {
                misses.push(format!(
                    "{method} {instance} as {who}: expected {expect}, answered {status}"
                ));
            }
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}
