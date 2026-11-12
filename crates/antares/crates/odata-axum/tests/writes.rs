// SPDX-License-Identifier: MIT
//! Writes through the read filter on every source (T-6659): one `$filter` answers the same rows
//! as an internal `query`, an OData GET, a filtered PATCH dry run and a filtered DELETE on
//! `MemoryTable`, `GitTable` (on a branch) and `PgTable`; a write reaching a row outside what the
//! caller may change changes nothing.
//!
//! `PgTable` runs when `ANTARES_TEST_DATABASE_URL` names a PostgreSQL server, and fails (never
//! skips) when the variable is set and the server is unreachable.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use odata_axum::{table_router, ODataState};
use odata_table::{
    query, Batch, Change, Cond, EntityDef, Field, FieldKind, GitLimits, GitTable, MemoryTable,
    Operation, PgTable, Rule, Scope, Service, Table, TableError,
};
use odata_url::{EdmModel, Limits, ODataUri, QueryOptions};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

fn service() -> Service {
    let rule = |operation, permission: &str| Rule {
        operation,
        permission: Some(permission.to_owned()),
    };
    let mut id = Field::new("id", FieldKind::String, "The key, e.g. t1");
    id.nullable = false;
    Service {
        namespace: "Sample".into(),
        title: "Sample sets".into(),
        sets: vec![EntityDef {
            set: "Things".into(),
            type_name: "Thing".into(),
            description: "A thing an organization owns".into(),
            key: "id".into(),
            fields: vec![
                id,
                Field::new("org", FieldKind::String, "The owning organization, e.g. a"),
                Field::new("name", FieldKind::String, "Its name, e.g. Pump"),
                Field::new("size", FieldKind::Integer, "Its size, e.g. 3"),
            ],
            rules: vec![
                rule(Operation::Read, "things.read"),
                rule(Operation::Export, "things.export"),
                rule(Operation::Update, "things.update"),
                rule(Operation::Delete, "things.delete"),
            ],
        }],
    }
}

/// Organizations a and b readable, a changeable; c is another tenant's.
fn rows() -> Vec<Value> {
    vec![
        json!({"id": "t1", "org": "a", "name": "Pump", "size": 10}),
        json!({"id": "t2", "org": "a", "name": "Valve", "size": 7}),
        json!({"id": "t3", "org": "b", "name": "Gauge", "size": 3}),
        json!({"id": "t4", "org": "b", "name": "Meter", "size": 1}),
        json!({"id": "t5", "org": "c", "name": "Tank", "size": 20}),
    ]
}

fn scope() -> Scope {
    Scope::all()
        .and(&["org"], vec![json!("a"), json!("b")])
        .changing(&["org"], vec![json!("a")])
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.org"])
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A repository with one file a row, YAML and JSON mixed, on branch `main`.
fn repository() -> Result<(PathBuf, String), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let repo =
        std::env::temp_dir().join(format!("odata-axum-writes-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(repo.join("things")).map_err(|e| e.to_string())?;
    for (at, row) in rows().iter().enumerate() {
        let id = row["id"].as_str().ok_or("no id")?;
        let file = if at % 2 == 0 {
            format!("{id}.yaml")
        } else {
            format!("{id}.json")
        };
        std::fs::write(repo.join("things").join(file), row.to_string())
            .map_err(|e| e.to_string())?;
    }
    git(&repo, &["init", "-q", "-b", "main"])?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "rows"])?;
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    Ok((repo, sha))
}

fn state(service: &Service) -> ODataState {
    ODataState {
        model: Arc::new(service.model()),
        url_model: None,
        service_root: "https://example.org/odata/".into(),
        url_limits: Limits::strict(),
        csdl_limits: odata_csdl::Limits::strict(),
        max_header_len: 1024,
        metadata_etag: None,
        metadata: Arc::default(),
        custom_options: vec![],
    }
}

/// Every source over [`rows`]: memory and Git always, PostgreSQL when configured; the Git
/// source also answers its repository.
async fn sources(
    test: &str,
) -> Result<Vec<(&'static str, Arc<dyn Table>, Option<PathBuf>)>, String> {
    let service = service();
    let model = Arc::new(service.model());
    let info = || service.info(model.clone(), "Things").ok_or("no set");
    let (repo, sha) = repository()?;
    let git_table = GitTable::open(info()?, &repo, &sha, "things/*", GitLimits::strict())
        .and_then(|t| t.on_branch("main", "Writer", "writer@example.org"))
        .map_err(|e| e.to_string())?;
    let mut out: Vec<(&'static str, Arc<dyn Table>, Option<PathBuf>)> = vec![
        (
            "memory",
            Arc::new(MemoryTable::new(info()?, rows()).map_err(|e| e.to_string())?),
            None,
        ),
        ("git", Arc::new(git_table), Some(repo)),
    ];
    if let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") {
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .map_err(|e| format!("ANTARES_TEST_DATABASE_URL is set and unreachable: {e}"))?;
        let name = format!("odata_axum_writes_{test}_{}", std::process::id());
        for stmt in [
            format!("DROP TABLE IF EXISTS {name}"),
            format!("CREATE TABLE {name} (id text PRIMARY KEY, org text, name text, size bigint)"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(stmt))
                .execute(&pool)
                .await
                .map_err(|e| e.to_string())?;
        }
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {name} SELECT * FROM jsonb_populate_recordset(NULL::{name}, $1)"
        )))
        .bind(Value::Array(rows()))
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;
        out.push((
            "postgres",
            Arc::new(PgTable::new(info()?, pool, &name).map_err(|e| e.to_string())?),
            None,
        ));
    }
    Ok(out)
}

fn app(table: Arc<dyn Table>, scope: Scope) -> Router {
    let service = service();
    Router::new()
        .nest_service(
            "/odata",
            table_router(Arc::new(state(&service)), vec![table], 2),
        )
        .layer(Extension(scope))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value), String> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .map_err(|e| e.to_string())?;
    let Ok(response) = app.clone().oneshot(request).await;
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

fn options(query: &str) -> Result<QueryOptions, String> {
    let model = service().model();
    let uri = odata_url::parse(
        &format!("Things?{query}"),
        &EdmModel(&model),
        &Limits::strict(),
    )
    .map_err(|e| format!("{query}: {e:?}"))?;
    let ODataUri::Resource { options, .. } = uri else {
        return Err(format!("{query}: not a resource"));
    };
    Ok(options)
}

fn ids(values: &Value) -> Vec<String> {
    let mut ids: Vec<String> = values
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.get("id").or(Some(v)).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids
}

/// Every row of the table, whoever may read it, by id.
async fn everything(table: &dyn Table) -> Result<Vec<Value>, String> {
    let answer = query(table, &options("$orderby=id")?, &Scope::all(), 100)
        .await
        .map_err(|e| e.to_string())?;
    Ok(answer.rows)
}

#[tokio::test]
async fn one_filter_reads_and_writes_the_same_rows_on_every_source() -> Result<(), String> {
    const FILTER: &str = "$filter=size%20gt%205";
    let want = vec!["t1".to_owned(), "t2".to_owned()];
    for (source, table, repo) in sources("same_rows").await? {
        let app = app(table.clone(), scope());
        // an internal call: module code passing the parsed filter, no HTTP
        let internal = query(table.as_ref(), &options(FILTER)?, &scope(), 100)
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(ids(&Value::Array(internal.rows)), want, "{source}: query");
        let (status, read) = call(&app, "GET", &format!("/odata/Things?{FILTER}"), None).await?;
        assert_eq!(status, StatusCode::OK, "{source}: GET {read}");
        assert_eq!(ids(&read["value"]), want, "{source}: GET");
        let before = everything(table.as_ref()).await?;
        let (status, dry) = call(
            &app,
            "PATCH",
            &format!("/odata/Things?{FILTER}&dryRun=true"),
            Some(json!({"name": "Renamed"})),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "{source}: dry run {dry}");
        assert_eq!(
            (dry["dryRun"].clone(), dry["changed"].clone()),
            (json!(true), json!(2)),
            "{source}"
        );
        assert_eq!(ids(&dry["keys"]), want, "{source}: dry run keys");
        assert!(
            dry["value"]
                .as_array()
                .is_some_and(|v| v.iter().all(|r| r["name"] == "Renamed")),
            "{source}: the dry run answers the rows as they would be: {dry}"
        );
        assert_eq!(
            everything(table.as_ref()).await?,
            before,
            "{source}: a dry run changed rows"
        );
        let (status, gone) = call(&app, "DELETE", &format!("/odata/Things?{FILTER}"), None).await?;
        assert_eq!(status, StatusCode::OK, "{source}: DELETE {gone}");
        assert_eq!(ids(&gone["keys"]), want, "{source}: DELETE keys");
        assert_eq!(ids(&gone["value"]), want, "{source}: DELETE rows");
        assert_eq!(
            ids(&Value::Array(everything(table.as_ref()).await?)),
            ["t3", "t4", "t5"],
            "{source}: after DELETE"
        );
        if let Some(repo) = repo {
            let files = git(&repo, &["ls-tree", "-r", "--name-only", "main"])?;
            assert!(
                !files.contains("t1.") && !files.contains("t2.") && files.contains("t3."),
                "{source}: the branch still holds the deleted files: {files}"
            );
            assert_eq!(
                git(&repo, &["log", "-1", "--format=%an %s", "main"])?,
                "Writer delete 2 of Things"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn an_update_writes_through_and_reads_back_on_every_source() -> Result<(), String> {
    for (source, table, repo) in sources("update").await? {
        let app = app(table.clone(), scope());
        let (status, done) = call(
            &app,
            "PATCH",
            "/odata/Things?$filter=org%20eq%20'a'",
            Some(json!({"name": "Renamed", "size": 11})),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "{source}: {done}");
        assert_eq!(done["dryRun"], json!(false), "{source}");
        let (_, read) = call(
            &app,
            "GET",
            "/odata/Things?$filter=name%20eq%20'Renamed'",
            None,
        )
        .await?;
        assert_eq!(ids(&read["value"]), ["t1", "t2"], "{source}: read back");
        assert!(
            read["value"]
                .as_array()
                .is_some_and(|v| v.iter().all(|r| r["size"] == 11)),
            "{source}: {read}"
        );
        if let Some(repo) = repo {
            let t1 = git(&repo, &["show", "main:things/t1.yaml"])?;
            assert!(t1.contains("Renamed"), "{source}: {t1}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_write_reaching_a_row_outside_what_the_caller_may_change_changes_nothing(
) -> Result<(), String> {
    for (source, table, _) in sources("outside").await? {
        let app = app(table.clone(), scope());
        let before = everything(table.as_ref()).await?;
        for (method, uri, body) in [
            // org b: readable, not changeable
            (
                "PATCH",
                "/odata/Things?$filter=size%20lt%205",
                Some(json!({"name": "x"})),
            ),
            ("DELETE", "/odata/Things?$filter=size%20lt%205", None),
            // a and b together: the whole write is refused, the org a rows too
            ("DELETE", "/odata/Things?$filter=size%20gt%202", None),
            (
                "PATCH",
                "/odata/Things?$filter=size%20gt%205%20or%20true",
                Some(json!({"name": "x"})),
            ),
            ("DELETE", "/odata/Things('t3')", None),
            // moving a changeable row out of reach
            ("PATCH", "/odata/Things('t1')", Some(json!({"org": "b"}))),
            (
                "PATCH",
                "/odata/Things?$filter=org%20eq%20'a'",
                Some(json!({"org": "c"})),
            ),
        ] {
            let (status, body) = call(&app, method, uri, body).await?;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{source}: {method} {uri} {body}"
            );
        }
        // another tenant's row answers as a missing one
        for (method, body) in [("PATCH", Some(json!({"name": "x"}))), ("DELETE", None)] {
            let (status, _) = call(&app, method, "/odata/Things('t5')", body).await?;
            assert_eq!(status, StatusCode::NOT_FOUND, "{source}: {method} t5");
        }
        // a filter reaching only another tenant's rows matches nothing
        let (status, none) =
            call(&app, "DELETE", "/odata/Things?$filter=org%20eq%20'c'", None).await?;
        assert_eq!(
            (status, none["changed"].clone()),
            (StatusCode::OK, json!(0)),
            "{source}"
        );
        assert_eq!(
            everything(table.as_ref()).await?,
            before,
            "{source}: a refused write changed rows"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_write_is_refused_without_the_permission_or_with_a_bad_body() -> Result<(), String> {
    let (_, table, _) = sources("refused")
        .await?
        .into_iter()
        .next()
        .ok_or("no source")?;
    let reader = app(table.clone(), scope().holding(&["things.read"]));
    let (status, body) = call(
        &reader,
        "DELETE",
        "/odata/Things?$filter=size%20gt%205",
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.to_string().contains("things.delete"), "{body}");
    let writer = app(table.clone(), scope());
    for (uri, patch, why) in [
        (
            "/odata/Things?$filter=size%20gt%205",
            json!({"id": "t9"}),
            "key",
        ),
        (
            "/odata/Things?$filter=size%20gt%205",
            json!({"colour": "red"}),
            "unknown",
        ),
        (
            "/odata/Things?$filter=size%20gt%205",
            json!({"size": "big"}),
            "type",
        ),
        ("/odata/Things?$filter=size%20gt%205", json!({}), "empty"),
        (
            "/odata/Things?$filter=size%20gt%205",
            json!([1]),
            "not an object",
        ),
        (
            "/odata/Things?$top=1",
            json!({"name": "x"}),
            "an option other than $filter",
        ),
        (
            "/odata/Things?$filter=size%20gt%205&dryRun=maybe",
            json!({"name": "x"}),
            "dryRun",
        ),
    ] {
        let (status, body) = call(&writer, "PATCH", uri, Some(patch)).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    }
    assert_eq!(everything(table.as_ref()).await?, rows());
    Ok(())
}

#[tokio::test]
async fn a_source_refuses_a_batch_whose_rows_moved_and_changes_nothing() -> Result<(), String> {
    for (source, table, _) in sources("moved").await? {
        let before = everything(table.as_ref()).await?;
        for change in [
            Change::Delete,
            Change::Update(serde_json::Map::from_iter([(
                "name".to_owned(),
                json!("x"),
            )])),
        ] {
            let batch = Batch {
                change,
                keys: vec![json!("t1"), json!("t2")],
                // t1 still meets it, t2 no longer does
                guard: odata_table::plan(table.info(), &options("$filter=size%20gt%208")?, 1)
                    .map_err(|e| e.to_string())?
                    .filter,
            };
            let refused = table.apply(&batch).await;
            assert!(
                matches!(refused, Err(TableError::Conflict(_))),
                "{source}: {refused:?}"
            );
            let gone = table
                .apply(&Batch {
                    keys: vec![json!("nope")],
                    guard: Cond::Const(true),
                    ..batch
                })
                .await;
            assert!(
                matches!(gone, Err(TableError::Conflict(_))),
                "{source}: {gone:?}"
            );
        }
        assert_eq!(everything(table.as_ref()).await?, before, "{source}");
    }
    Ok(())
}

#[tokio::test]
async fn a_git_write_refuses_a_branch_someone_else_moved() -> Result<(), String> {
    let (_, table, repo) = sources("branch")
        .await?
        .into_iter()
        .find(|(s, _, _)| *s == "git")
        .ok_or("no git source")?;
    let repo = repo.ok_or("no repository")?;
    let read = table.etag().ok_or("no etag")?;
    std::fs::write(repo.join("other.txt"), "another writer").map_err(|e| e.to_string())?;
    git(&repo, &["add", "other.txt"])?;
    git(&repo, &["commit", "-q", "-m", "another writer"])?;
    let theirs = git(&repo, &["rev-parse", "main"])?;
    let app = app(table.clone(), scope());
    let (status, body) = call(&app, "DELETE", "/odata/Things('t1')", None).await?;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        git(&repo, &["rev-parse", "main"])?,
        theirs,
        "their commit was lost"
    );
    assert_eq!(table.etag(), Some(read));
    assert_eq!(everything(table.as_ref()).await?, rows());
    // a set opened read only, or on a branch past its commit, takes no write
    let service = service();
    let info = service
        .info(Arc::new(service.model()), "Things")
        .ok_or("no set")?;
    let stale = GitTable::open(
        info,
        &repo,
        &read_commit(&repo)?,
        "things/*",
        GitLimits::strict(),
    )
    .map_err(|e| e.to_string())?;
    assert!(!stale.writable());
    assert!(stale.on_branch("main", "W", "w@example.org").is_err());
    Ok(())
}

/// The first commit of `repo`'s main branch.
fn read_commit(repo: &Path) -> Result<String, String> {
    git(repo, &["rev-list", "--max-parents=0", "main"])
}
