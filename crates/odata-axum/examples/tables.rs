// SPDX-License-Identifier: MIT
//! Two entity sets from one `Service` definition, served by `table_router` on
//! `127.0.0.1:<port>/odata/`: `Things` from memory, or `Files` from a Git repository at one
//! commit when one is given (one set per run: the .NET check reads a service's first set). For the conformance suites (joinedcontext-conformance tests/odata) and the
//! .NET client check; `openapi` writes the OpenAPI 3.1 document of the same definition.
//!
//! ```sh
//! cargo run -p odata-axum --example tables -- serve 8090
//! cargo run -p odata-axum --example tables -- serve 8090 <repository> <commit> 'files/*.yaml'
//! cargo run -p odata-axum --example tables -- openapi openapi.json
//! ```

use std::sync::Arc;

use odata_axum::{metadata_etag, table_router, ODataState};
use odata_table::{
    open_rules, EntityDef, Field, FieldKind, GitLimits, GitTable, MemoryTable, Scope, Service,
    Table,
};
use serde_json::json;

fn service() -> Service {
    // a key is never null (CSDL 4.01 8.2)
    let mut id = Field::new("id", FieldKind::String, "The key, e.g. t1");
    id.nullable = false;
    let set = |set: &str, type_name: &str, description: &str, fields: Vec<Field>| EntityDef {
        set: set.into(),
        type_name: type_name.into(),
        description: description.into(),
        key: "id".into(),
        fields,
        rules: open_rules(),
    };
    Service {
        namespace: "Example".into(),
        title: "odata-table example".into(),
        sets: vec![
            set(
                "Things",
                "Thing",
                "A sample row held in memory",
                vec![
                    id.clone(),
                    Field::new("name", FieldKind::String, "Its name, e.g. Alpha"),
                    Field::new("size", FieldKind::Integer, "Its size, e.g. 3"),
                ],
            ),
            set(
                "Files",
                "File",
                "A YAML or JSON file of a Git repository",
                vec![
                    id,
                    Field::new("title", FieldKind::String, "Its title, e.g. First"),
                ],
            ),
        ],
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage =
        "usage: tables serve <port> [<repository> <commit> <pattern>] | tables openapi <file>";
    let mut service = service();
    if let [mode, file] = args.as_slice() {
        if mode == "openapi" {
            let doc = service.openapi("http://127.0.0.1:8090/odata/");
            return std::fs::write(file, doc.to_string()).map_err(|e| e.to_string());
        }
    }
    let (port, git) = match args.as_slice() {
        [mode, port] if mode == "serve" => (port, None),
        [mode, port, repo, commit, pattern] if mode == "serve" => {
            (port, Some((repo, commit, pattern)))
        }
        _ => return Err(usage.into()),
    };
    let port: u16 = port.parse().map_err(|_| usage)?;
    let served = if git.is_some() { "Files" } else { "Things" };
    service.sets.retain(|s| s.set == served);
    let model = Arc::new(service.model());
    let info = |set| {
        service
            .info(model.clone(), set)
            .ok_or(format!("no set {set}"))
    };
    let table: Arc<dyn Table> = match git {
        Some((repo, commit, pattern)) => Arc::new(
            GitTable::open(info("Files")?, repo, commit, pattern, GitLimits::strict())
                .map_err(|e| e.to_string())?,
        ),
        None => Arc::new(
            MemoryTable::new(
                info("Things")?,
                vec![
                    json!({"id": "t1", "name": "Alpha", "size": 3}),
                    json!({"id": "t2", "name": "Beta", "size": 1}),
                ],
            )
            .map_err(|e| e.to_string())?,
        ),
    };
    let csdl_limits = odata_csdl::Limits::strict();
    let state = ODataState {
        metadata_etag: metadata_etag(&model, &csdl_limits),
        model,
        url_model: None,
        service_root: format!("http://127.0.0.1:{port}/odata/"),
        url_limits: odata_url::Limits::strict(),
        csdl_limits,
        max_header_len: 1024,
        metadata: Arc::default(),
        custom_options: vec![],
    };
    let app = axum::Router::new()
        .nest_service("/odata", table_router(Arc::new(state), vec![table], 100))
        // an example with public rows: every request reads every row
        .layer(axum::Extension(Scope::all()));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| e.to_string())?;
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}
