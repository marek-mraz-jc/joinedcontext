// SPDX-License-Identifier: MIT
//! One `Service` definition serving a `MemoryTable` and a `GitTable` (T-6642 owner addition):
//! `$metadata`, OpenAPI 3.1 with the same fields, `$export` and `$allowed` agreeing with the
//! real call.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use odata_axum::{table_router, ODataState};
use odata_table::{
    EntityDef, Field, FieldKind, GitLimits, GitTable, MemoryTable, Operation, Rule, Scope, Service,
    Table,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn service() -> Service {
    let rule = |operation, permission: &str| Rule {
        operation,
        permission: Some(permission.to_owned()),
    };
    let mut id = Field::new("id", FieldKind::String, "The key, e.g. t1");
    id.nullable = false;
    let address = FieldKind::Object {
        name: "Address".into(),
        fields: vec![Field::new(
            "city",
            FieldKind::String,
            "The town, e.g. Zilina",
        )],
    };
    Service {
        namespace: "Sample".into(),
        title: "Sample sets".into(),
        sets: vec![
            EntityDef {
                set: "Things".into(),
                type_name: "Thing".into(),
                description: "A thing an organization owns".into(),
                key: "id".into(),
                fields: vec![
                    id.clone(),
                    Field::new("org", FieldKind::String, "The owning organization, e.g. a"),
                    Field::new("size", FieldKind::Integer, "Its size, e.g. 3"),
                    Field::new("address", address, "Where it is"),
                ],
                rules: vec![
                    rule(Operation::Read, "things.read"),
                    rule(Operation::Export, "things.export"),
                    rule(Operation::Update, "things.update"),
                ],
            },
            EntityDef {
                set: "Files".into(),
                type_name: "File".into(),
                description: "A file of the repository".into(),
                key: "id".into(),
                fields: vec![id, Field::new("title", FieldKind::String, "Its title")],
                rules: odata_table::open_rules(),
            },
        ],
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn app(scope: Scope) -> Result<Router, String> {
    let service = service();
    let model = Arc::new(service.model());
    let info = |set| {
        service
            .info(model.clone(), set)
            .ok_or(format!("no set {set}"))
    };
    let things = MemoryTable::new(
        info("Things")?,
        vec![
            json!({"id": "t1", "org": "a", "size": 3, "address": {"city": "Zilina"}}),
            json!({"id": "t2", "org": "b", "size": 1}),
            json!({"id": "t3", "org": "a", "size": 2}),
        ],
    )
    .map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let repo =
        std::env::temp_dir().join(format!("odata-axum-service-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(repo.join("files")).map_err(|e| e.to_string())?;
    std::fs::write(repo.join("files/f1.yaml"), "id: f1\ntitle: First\n")
        .map_err(|e| e.to_string())?;
    git(&repo, &["init", "-q"])?;
    git(&repo, &["add", "-A"])?;
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.org",
            "commit",
            "-q",
            "-m",
            "f",
        ],
    )?;
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    let files = GitTable::open(
        info("Files")?,
        &repo,
        &sha,
        "files/*.yaml",
        GitLimits::strict(),
    )
    .map_err(|e| e.to_string())?;
    let state = ODataState {
        model,
        url_model: None,
        service_root: "https://example.org/odata/".into(),
        url_limits: odata_url::Limits::strict(),
        csdl_limits: odata_csdl::Limits::strict(),
        max_header_len: 1024,
        metadata_etag: None,
        metadata: Arc::default(),
        custom_options: vec![],
    };
    let tables: Vec<Arc<dyn Table>> = vec![Arc::new(things), Arc::new(files)];
    Ok(Router::new()
        .nest_service("/odata", table_router(Arc::new(state), tables, 2))
        .layer(Extension(scope)))
}

async fn call(app: &Router, method: &str, uri: &str) -> Result<(StatusCode, Value), String> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

#[tokio::test]
async fn openapi_and_csdl_name_the_same_fields_and_both_sets_serve() -> Result<(), String> {
    let service = service();
    let model = service.model();
    let openapi = service.openapi("https://example.org/odata/");
    assert_eq!(openapi["openapi"], "3.1.0");
    for def in &service.sets {
        let ty = model
            .entity_type(&format!("Sample.{}", def.type_name))
            .ok_or("no entity type")?;
        let csdl: Vec<&str> = ty.properties.iter().map(|p| p.name.as_str()).collect();
        let schema = openapi["components"]["schemas"][&def.type_name]["properties"]
            .as_object()
            .ok_or("no schema")?;
        let mut api: Vec<&str> = schema.keys().map(String::as_str).collect();
        let mut csdl_sorted = csdl.clone();
        api.sort_unstable();
        csdl_sorted.sort_unstable();
        assert_eq!(api, csdl_sorted, "{}", def.set);
        for path in ["", "({key})", "/$count", "/$export", "/$allowed"] {
            assert!(
                openapi["paths"]
                    .get(format!("/{}{path}", def.set))
                    .is_some(),
                "{}{path}",
                def.set
            );
        }
    }
    let app = app(Scope::all())?;
    let (status, _) = call(&app, "GET", "/odata/$metadata").await?;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&app, "GET", "/odata/Files").await?;
    assert_eq!(
        (status, body["value"].clone()),
        (StatusCode::OK, json!([{"id": "f1", "title": "First"}]))
    );
    Ok(())
}

#[tokio::test]
async fn allowed_answers_what_the_real_call_does() -> Result<(), String> {
    let scopes = [
        ("all", Scope::all()),
        (
            "org a reader",
            Scope::all()
                .and(&["org"], vec![json!("a")])
                .holding(&["things.read", "things.export"]),
        ),
        ("exporter only", Scope::all().holding(&["things.export"])),
        ("nothing held", Scope::default()),
    ];
    // (operation, key, the real call)
    let calls = [
        ("read", None, "GET", "/odata/Things"),
        ("read", Some("t1"), "GET", "/odata/Things('t1')"),
        ("read", Some("t2"), "GET", "/odata/Things('t2')"),
        ("read", Some("nope"), "GET", "/odata/Things('nope')"),
        ("export", None, "GET", "/odata/Things/$export"),
        ("update", Some("t1"), "PATCH", "/odata/Things('t1')"),
        ("delete", Some("t1"), "DELETE", "/odata/Things('t1')"),
    ];
    for (who, scope) in scopes {
        let app = app(scope)?;
        for (op, key, method, uri) in calls {
            let ask = match key {
                Some(k) => format!("/odata/Things/$allowed?op={op}&key={k}"),
                None => format!("/odata/Things/$allowed?op={op}"),
            };
            let (status, decision) = call(&app, "GET", &ask).await?;
            assert_eq!(status, StatusCode::OK, "{who} {ask}: {decision}");
            let (real, body) = call(&app, method, uri).await?;
            assert_eq!(
                decision["allowed"].as_bool(),
                Some(real == StatusCode::OK),
                "{who}: {ask} said {decision}, {method} {uri} answered {real} {body}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn allowed_never_tells_a_hidden_row_from_a_missing_one() -> Result<(), String> {
    let app = app(Scope::all()
        .and(&["org"], vec![json!("a")])
        .holding(&["things.read", "things.update"]))?;
    for op in ["read", "update"] {
        let (_, hidden) = call(
            &app,
            "GET",
            &format!("/odata/Things/$allowed?op={op}&key=t2"),
        )
        .await?;
        let (_, missing) = call(
            &app,
            "GET",
            &format!("/odata/Things/$allowed?op={op}&key=zz"),
        )
        .await?;
        assert_eq!(hidden, missing, "{op}");
        assert_eq!(hidden["allowed"], false, "{op}");
    }
    let (_, decision) = call(&app, "GET", "/odata/Things/$allowed?op=export").await?;
    assert_eq!(
        decision,
        json!({"allowed": false, "reason": "export needs the permission things.export", "permission": "things.export"})
    );
    for bad in [
        "/odata/Things/$allowed",
        "/odata/Things/$allowed?op=fly",
        "/odata/Things/$allowed?op=read&x=1",
    ] {
        let (status, _) = call(&app, "GET", bad).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    Ok(())
}

#[tokio::test]
async fn export_holds_only_the_rows_of_the_scope() -> Result<(), String> {
    let narrow = app(Scope::all()
        .and(&["org"], vec![json!("a")])
        .holding(&["things.export"]))?;
    let (status, rows) = call(&narrow, "GET", "/odata/Things/$export?$orderby=size%20desc").await?;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(
        rows,
        json!([
            {"id": "t1", "org": "a", "size": 3, "address": {"city": "Zilina"}},
            {"id": "t3", "org": "a", "size": 2, "address": null}
        ]),
        "pages of 2 rows joined, org b left out"
    );
    let (_, rows) = call(
        &narrow,
        "GET",
        "/odata/Things/$export?$filter=org%20eq%20'b'%20or%20true&$select=id",
    )
    .await?;
    assert_eq!(rows, json!([{"id": "t1"}, {"id": "t3"}]));
    let reader = app(Scope::all().holding(&["things.read"]))?;
    let (status, _) = call(&reader, "GET", "/odata/Things/$export").await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    Ok(())
}
