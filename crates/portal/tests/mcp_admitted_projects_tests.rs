//! A named MCP server admits only projects its proposer may read (T-4948): the form's picker lists
//! those alone, and the server checks every name again, so one typed through the API, of another
//! organization or of no project at all, is refused by the same answer naming the field.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, forge, person, send};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;

const SERVERS: &str = "/api/v1/projects/ovzdusie/mcpservers";
const CHECKED: &str = "/api/v1/projects/ovzdusie/mcpservers?dryRun=All";

/// The dry run's verdict: whether it passes, and the fields its findings name.
fn verdict(text: &str) -> (bool, Vec<String>) {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let fields = body["verdict"]["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str().map(str::to_owned))
        .collect();
    (body["verdict"]["ok"] == json!(true), fields)
}

fn server(name: &str, admitted: &[&str]) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "McpServer",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": {
            "members": [{ "kind": "Endpoint", "name": "air", "namespace": "ovzdusie" }],
            "audience": "project-list",
            "allowedProjects": admitted,
        },
    })
}

#[tokio::test]
async fn an_mcp_server_admits_only_projects_its_proposer_reads() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "mcp-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["McpServer", "Endpoint"], "verbs": ["propose", "read"] }] }),
    ));
    for project in ["ovzdusie", "doprava"] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("editor-in-{project}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "editor@hel.fi" }], "role": "mcp-editor",
                    "scope": { "project": project } }),
        ));
    }
    for project in ["ovzdusie", "doprava"] {
        state.mirror.upsert(envelope(
            "Endpoint",
            "air",
            project,
            json!({ "slug": format!("air-{project}"), "audience": "organization" }),
        ));
    }
    // `skolstvo` exists, but nothing binds the editor there.
    state.mirror.upsert(envelope(
        "Endpoint",
        "schools",
        "skolstvo",
        json!({ "slug": "sch1", "audience": "organization" }),
    ));

    let allowed = send(
        &state,
        person("editor"),
        "POST",
        CHECKED,
        Some(server("shared", &["doprava"])),
    )
    .await;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.text);
    assert_eq!(verdict(&allowed.text), (true, vec![]), "{}", allowed.text);
    let checked = send(
        &state,
        person("editor"),
        "POST",
        CHECKED,
        Some(server("shared", &["skolstvo"])),
    )
    .await;
    assert_eq!(
        verdict(&checked.text),
        (false, vec!["spec.allowedProjects".to_owned()]),
        "{}",
        checked.text
    );

    let mut answers = Vec::new();
    for project in ["skolstvo", "nosuch"] {
        let refused = send(
            &state,
            person("editor"),
            "POST",
            SERVERS,
            Some(server("shared", &["doprava", project])),
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{project}: {}",
            refused.text
        );
        let body: Value = serde_json::from_str(&refused.text).unwrap_or(Value::Null);
        assert_eq!(
            body["errors"][0]["path"],
            json!("spec.allowedProjects"),
            "{}",
            refused.text
        );
        answers.push(
            body["detail"]
                .as_str()
                .unwrap_or_default()
                .replace(project, "<p>"),
        );
    }
    assert_eq!(
        answers[0], answers[1],
        "an unreadable project reads like one that is not there"
    );

    // A project admitted before stays: editing the server keeps it without a read right.
    state.mirror.upsert(envelope(
        "McpServer",
        "kept",
        "ovzdusie",
        server("kept", &["skolstvo"])["spec"].clone(),
    ));
    let edited = send(
        &state,
        person("editor"),
        "PUT",
        "/api/v1/projects/ovzdusie/mcpservers/kept?dryRun=All",
        Some(server("kept", &["skolstvo", "doprava"])),
    )
    .await;
    assert_eq!(verdict(&edited.text), (true, vec![]), "{}", edited.text);
}
