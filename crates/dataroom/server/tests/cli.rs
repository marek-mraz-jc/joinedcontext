#![forbid(unsafe_code)]
//! The `dataroom` CLI: every subcommand through `run_cli` on the in-memory store and on
//! Postgres (migration owner for `migrate`, a login role without superuser or BYPASSRLS for the
//! rest), plus the binary's exit codes.

use clap::Parser;
use dataroom_core::{CatalogueId, DataroomError, OrgId, RequestCtx, Store, Subject};
use dataroom_server::{ensure_default_catalogue, import_ckan_json, run_cli, Cli};
use dataroom_store_memory::MemoryStore;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command as Process;
use uuid::Uuid;

fn cli(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("dataroom").chain(args.iter().copied())).expect("cli args")
}

fn temp_file(content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("dataroom-cli-{}.json", Uuid::new_v4()));
    std::fs::write(&path, content).unwrap();
    path
}

fn package(name: &str, org: &str) -> Value {
    json!({
        "name": name, "title": format!("Dataset {name}"), "notes": "Popis",
        "license_id": "cc-by", "owner_org": org,
        "resources": [{"name": "CSV", "url": format!("https://example.org/{name}.csv"), "format": "CSV"}]
    })
}

#[tokio::test]
async fn memory_store_commands_answer_with_json_summaries() {
    let out = run_cli(cli(&["migrate"])).await.unwrap();
    assert_eq!(out, json!({"migrated": true, "store": "memory"}));

    let out = run_cli(cli(&["feed-check"])).await.unwrap();
    assert!(out.is_object(), "{out}");
    let out = run_cli(cli(&["feed-check", "--profile", "dcat-ap-3.0.1"]))
        .await
        .unwrap();
    assert!(out.is_object(), "{out}");

    let dump = temp_file(&json!([package("ovzdusie", "mesto")]).to_string());
    let out = run_cli(cli(&["import-ckan", "--file", dump.to_str().unwrap()]))
        .await
        .unwrap();
    std::fs::remove_file(&dump).unwrap();
    assert_eq!(out, json!({"publishersCreated": 1, "datasetsImported": 1}));

    // The CLI operator holds Admin on its catalogue: the registry runs with that grant.
    let out = run_cli(cli(&["op", "--name", "catalogue.get"]))
        .await
        .unwrap();
    assert_eq!(out["host"], "opendata.localhost", "{out}");
    let out = run_cli(cli(&[
        "op",
        "--name",
        "jc_publisher_propose",
        "--input",
        r#"{"name":"mesto","title":{"sk":"Mesto"},"uri":"https://example.org/p/mesto","state":"active"}"#,
    ]))
    .await
    .unwrap();
    assert_eq!(out["name"], "mesto", "{out}");
}

#[tokio::test]
async fn memory_store_commands_refuse_bad_input() {
    let err = run_cli(cli(&["--org-id", "not-a-uuid", "migrate"]))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            DataroomError::Validation { .. } | DataroomError::Forbidden(_)
        ),
        "{err:?}"
    );
    let err = run_cli(cli(&["--catalogue-id", "nope", "migrate"]))
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());

    let err = run_cli(cli(&["harvest", "--source", "missing"]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DataroomError::NotFound(ref m) if m == "harvest source 'missing' not found"),
        "{err:?}"
    );

    let missing =
        std::env::temp_dir().join(format!("dataroom-cli-missing-{}.json", Uuid::new_v4()));
    let err = run_cli(cli(&["import-ckan", "--file", missing.to_str().unwrap()]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DataroomError::Storage(ref m) if m.starts_with("failed to read")),
        "{err:?}"
    );

    let bad = temp_file("{ not json");
    let err = run_cli(cli(&["import-ckan", "--file", bad.to_str().unwrap()]))
        .await
        .unwrap_err();
    std::fs::remove_file(&bad).unwrap();
    assert!(err.to_string().contains("invalid CKAN JSON"), "{err}");

    let err = run_cli(cli(&["op", "--name", "catalogue.get", "--input", "{oops"]))
        .await
        .unwrap_err();
    match err {
        DataroomError::Validation { fields, .. } => {
            assert!(
                fields
                    .get("input")
                    .is_some_and(|m| m.iter().any(|s| s.starts_with("invalid JSON"))),
                "{fields:?}"
            )
        }
        other => panic!("{other:?}"),
    }
    let err = run_cli(cli(&["op", "--name", "no.such.op"]))
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::NotFound(_)), "{err:?}");

    // Nothing listens on port 1: the connect error is a storage error, not a panic.
    let err = run_cli(cli(&[
        "--database-url",
        "postgres://nobody@127.0.0.1:1/none",
        "migrate",
    ]))
    .await
    .unwrap_err();
    assert!(
        matches!(err, DataroomError::Storage(ref m) if m.starts_with("connect postgres")),
        "{err:?}"
    );
}

#[tokio::test]
async fn serve_answers_on_its_host_and_reports_a_taken_port() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = held.local_addr().unwrap().to_string();
    let err = run_cli(cli(&["serve", "--listen", &taken]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DataroomError::Storage(ref m) if m.starts_with(&format!("bind {taken}"))),
        "{err:?}"
    );
    drop(held);

    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
    let listen = addr.to_string();
    let server = tokio::spawn(run_cli(cli(&[
        "serve",
        "--listen",
        &listen,
        "--host",
        "cli.example.org",
    ])));
    let mut stream = None;
    for _ in 0..100 {
        if let Ok(s) = tokio::net::TcpStream::connect(addr).await {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let mut stream = stream.expect("server listens");
    stream
        .write_all(b"GET /api/3/action/package_list HTTP/1.1\r\nHost: cli.example.org\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.contains("\"success\":true"), "{answer}");
    server.abort();
}

#[tokio::test]
async fn import_ckan_json_reads_every_dump_shape() {
    let org = OrgId::from_uuid(Uuid::from_u128(0xc11));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(0xc12));
    let ctx = RequestCtx::new(
        org,
        cat,
        Subject::authenticated("admin", vec![org], Vec::new()),
    );

    for (dump, name) in [
        (
            json!({"package_search": {"result": {"results": [package("a1", "o1")]}}}),
            "a1",
        ),
        (json!({"results": [package("a2", "o1")]}), "a2"),
        (json!({"packages": [package("a3", "o1")]}), "a3"),
        (package("a4", "o1"), "a4"),
        (json!([package("a5", "o1")]), "a5"),
    ] {
        let store = MemoryStore::new();
        ensure_default_catalogue(&store, org, cat, "import.example.org")
            .await
            .unwrap();
        let (pubs, dss) = import_ckan_json(&store, &ctx, &dump.to_string(), "sk")
            .await
            .unwrap();
        assert_eq!((pubs, dss), (1, 1), "{dump}");
        let ds = store.get_dataset(&ctx, name).await.unwrap().expect(name);
        assert_eq!(ds.publisher_id, "o1");
    }

    // Organization entries as plain names or objects; empty names and other values are skipped;
    // a hidden list as an array; a package without an organization goes to the default publisher.
    let store = MemoryStore::new();
    ensure_default_catalogue(&store, org, cat, "import.example.org")
        .await
        .unwrap();
    // A second call keeps the existing catalogue.
    ensure_default_catalogue(&store, org, cat, "other.example.org")
        .await
        .unwrap();
    let dump = json!({
        "organizations": ["mesto", {"name": "kraj", "title": "Kraj", "image_url": "https://example.org/kraj.png"},
                          {"name": ""}, {"title": "no name"}, 42],
        "hidden": ["skryty"],
        "result": {"results": [{"name": "bez-org", "title": "Bez organizácie"}, package("skryty", "mesto")]}
    });
    let (pubs, dss) = import_ckan_json(&store, &ctx, &dump.to_string(), "sk")
        .await
        .unwrap();
    assert_eq!((pubs, dss), (3, 2));
    let kraj = store.get_publisher(&ctx, "kraj").await.unwrap().unwrap();
    assert_eq!(
        kraj.logo_url.as_deref(),
        Some("https://example.org/kraj.png")
    );
    assert!(kraj.description.is_empty());
    assert_eq!(
        store
            .get_publisher(&ctx, "mesto")
            .await
            .unwrap()
            .unwrap()
            .title["sk"],
        "mesto"
    );
    assert_eq!(
        store
            .get_dataset(&ctx, "bez-org")
            .await
            .unwrap()
            .unwrap()
            .publisher_id,
        "default-publisher"
    );
    let catalogue = store.get_catalogue(org, cat).await.unwrap().unwrap();
    assert_eq!(catalogue.host, "import.example.org");
    assert_eq!(
        catalogue.hidden_datasets.get("skryty").map(String::as_str),
        Some("hidden")
    );

    // Importing the same dump again updates the records instead of failing on their names.
    let first = store.get_dataset(&ctx, "skryty").await.unwrap().unwrap();
    let mut again = package("skryty", "mesto");
    again["title"] = json!("Nový názov");
    let (pubs, dss) = import_ckan_json(&store, &ctx, &json!([again]).to_string(), "sk")
        .await
        .unwrap();
    assert_eq!((pubs, dss), (0, 1));
    let second = store.get_dataset(&ctx, "skryty").await.unwrap().unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(
        second.title.get("sk").map(String::as_str),
        Some("Nový názov")
    );

    // A hidden value that is neither list nor map changes nothing; a dump with no packages fails.
    let err = import_ckan_json(&store, &ctx, r#"{"hidden": "x"}"#, "sk")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "validation error: expected CKAN package array or package_search envelope"
    );
}

/// The test database as `user` with `password` instead of the migration owner.
fn url_as(base: &str, user: &str, password: &str) -> String {
    let (scheme, rest) = base.split_once("://").expect("url scheme");
    let host_part = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    format!("{scheme}://{user}:{password}@{host_part}")
}

#[tokio::test]
async fn postgres_commands_run_as_a_login_role_and_refuse_the_owner() {
    let Some(owner_url) = std::env::var("DATAROOM_TEST_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        assert!(
            std::env::var("DATAROOM_REQUIRE_PG").ok().as_deref() != Some("1"),
            "DATAROOM_REQUIRE_PG=1 but DATAROOM_TEST_DATABASE_URL is unset"
        );
        println!("SKIP postgres_commands_run_as_a_login_role_and_refuse_the_owner: DATAROOM_TEST_DATABASE_URL is unset");
        return;
    };

    // migrate runs as the owner and is idempotent.
    for _ in 0..2 {
        let out = run_cli(cli(&["--database-url", &owner_url, "migrate"]))
            .await
            .unwrap();
        assert_eq!(out, json!({"migrated": true}));
    }

    // Every other command refuses a superuser: forced RLS would not bind it (C1).
    let err = run_cli(cli(&["--database-url", &owner_url, "feed-check"]))
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("superuser"),
        "{err}"
    );

    // A login role like the deployment's request role, with a password made for this run.
    let role = "dataroom_cli_test";
    let password = Uuid::new_v4().simple().to_string();
    let admin = sqlx::PgPool::connect(&owner_url).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN
             IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') THEN
                 CREATE ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS;
             END IF;
         END $$;
         ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{password}';
         GRANT CONNECT ON DATABASE {db} TO {role};
         GRANT USAGE ON SCHEMA dataroom TO {role};
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO {role};",
        db = owner_url
            .rsplit('/')
            .next()
            .unwrap()
            .split('?')
            .next()
            .unwrap()
    )))
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let app_url = url_as(&owner_url, role, &password);
    let pg = |args: &[&str]| {
        let mut all = vec!["--database-url", app_url.as_str()];
        all.extend_from_slice(args);
        cli(&all)
    };

    // The default catalogue exists after the first command and is read back through RLS.
    let out = run_cli(pg(&["op", "--name", "catalogue.get"]))
        .await
        .unwrap();
    assert_eq!(out["host"], "opendata.localhost", "{out}");
    let out = run_cli(pg(&["feed-check"])).await.unwrap();
    assert!(out.is_object(), "{out}");

    let dump = temp_file(&json!([package("cli-pg-ovzdusie", "cli-pg-mesto")]).to_string());
    // Run twice: a re-import updates what the first one created.
    for _ in 0..2 {
        let out = run_cli(pg(&["import-ckan", "--file", dump.to_str().unwrap()]))
            .await
            .unwrap();
        assert_eq!(out["datasetsImported"], 1, "{out}");
    }
    std::fs::remove_file(&dump).unwrap();
    let out = run_cli(pg(&[
        "op",
        "--name",
        "dataset.get",
        "--input",
        r#"{"id":"cli-pg-ovzdusie"}"#,
    ]))
    .await
    .unwrap();
    assert!(out.to_string().contains("cli-pg-ovzdusie"), "{out}");

    // A harvest source on loopback is saved, and its run is refused by the outbound guard.
    let looped = json!({"name": "cli-pg-loop2", "kind": "dcat-ap", "url": "https://127.0.0.1/feed.ttl",
                        "publisherId": "cli-pg-mesto", "allowedHosts": ["127.0.0.1"]});
    run_cli(pg(&[
        "op",
        "--name",
        "harvest_source.propose",
        "--input",
        &looped.to_string(),
    ]))
    .await
    .unwrap();
    let err = run_cli(pg(&["harvest", "--source", "cli-pg-loop2"]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DataroomError::OutboundRefused(ref m) if m.contains("127.0.0.1")),
        "{err:?}"
    );

    let err = run_cli(pg(&["harvest", "--source", "cli-pg-none"]))
        .await
        .unwrap_err();
    assert!(matches!(err, DataroomError::NotFound(_)), "{err:?}");
}

#[test]
fn the_binary_prints_json_and_exits_by_result() {
    let bin = env!("CARGO_BIN_EXE_dataroom");
    let run = |args: &[&str]| {
        Process::new(bin)
            .args(args)
            .env_remove("DATAROOM_DATABASE_URL")
            .env_remove("DATAROOM_ORG_ID")
            .env_remove("DATAROOM_CATALOGUE_ID")
            .output()
            .unwrap()
    };
    let ok = run(&["migrate"]);
    assert!(ok.status.success());
    let printed: Value = serde_json::from_slice(&ok.stdout).unwrap();
    assert_eq!(printed, json!({"migrated": true, "store": "memory"}));

    let failed = run(&["--org-id", "nope", "migrate"]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed.stderr).starts_with("error: "));
    assert!(failed.stdout.is_empty());
}

/// `serve --portal-token-file`: the Portal's token writes through the CKAN Action API, and
/// without the file every token is refused (PORTAL-CALLS.md).
#[tokio::test]
async fn serve_binds_the_portal_publisher_only_from_its_token_file() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    let org_id = OrgId::from_uuid(Uuid::from_u128(0x5e));
    let cat_id = CatalogueId::from_uuid(Uuid::from_u128(0x5e0));
    let token = "portal-publisher-token-0123456789abcdef";
    let file = temp_file(&format!("{token}\n"));
    let create = |token: &str| {
        Request::post("/api/3/action/organization_create")
            .header("Host", "127.0.0.1")
            .header("Authorization", token)
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"name":"mesto","title":"Mesto"}"#))
            .unwrap()
    };
    let answer = |state| async move {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        ensure_default_catalogue(store.as_ref(), org_id, cat_id, "opendata.localhost")
            .await
            .unwrap();
        let state: dataroom_api::DataroomState = state_with(store, state);
        let res = dataroom_api::router(state)
            .oneshot(create(token))
            .await
            .unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<Value>(&body).unwrap())
    };
    fn state_with(store: Arc<dyn Store>, file: Option<PathBuf>) -> dataroom_api::DataroomState {
        let org_id = OrgId::from_uuid(Uuid::from_u128(0x5e));
        let cat_id = CatalogueId::from_uuid(Uuid::from_u128(0x5e0));
        dataroom_server::serve_state(store, org_id, cat_id, file.as_deref()).unwrap()
    }

    let (status, body) = answer(Some(file.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["name"], "mesto");
    let (status, body) = answer(None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["success"], false);

    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let missing = std::env::temp_dir().join(format!("dataroom-none-{}", Uuid::new_v4()));
    assert!(dataroom_server::serve_state(store, org_id, cat_id, Some(&missing)).is_err());
    std::fs::remove_file(file).unwrap();
}
