#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! S9-28 (T-7572) against a live PostgreSQL: a write of daiya and its audit row commit in one
//! transaction, so a refused audit row leaves no change, and the request role only appends rows.
//!
//! `DAIYA_TEST_DATABASE_URL` names an admin connection. Unset: prints `SKIP`.

use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::{
    GrafanaRole, InstallationId, InstanceId, OrgId, PolicyAuthorizer, RoleBinding, Subject,
};
use daiya_store_pg::connect_test_database;
use http::{Request, StatusCode};
use sqlx::AssertSqlSafe;
use std::sync::Arc;

#[tokio::test]
async fn a_write_commits_with_its_audit_row_or_neither_stands() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    // A database of its own: the test refuses every audit row in it.
    let server = sqlx::PgPool::connect(&url).await.expect("server");
    let name = format!("daiya_audit_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&server)
        .await
        .expect("create");
    let (base, _) = url.rsplit_once('/').expect("a database in the url");
    let db = connect_test_database(&format!("{base}/{name}"))
        .await
        .expect("database");
    let org = OrgId::generate();
    let pool = db.app.pool().clone();
    let state = DaiyaState::new(
        InstallationId::from_u128(1),
        org,
        InstanceId::from_u128(1),
        Arc::new(db.app),
        Arc::new(PolicyAuthorizer::new(vec![RoleBinding::from_grafana_role(
            org,
            "admin",
            GrafanaRole::Admin,
            None,
        )])),
    )
    .with_tokens(vec![TokenBinding::new(
        "admin",
        Subject::authenticated("admin", vec![org], vec![]),
    )]);
    let app = api_router(
        state.clone(),
        jc_router::Mount::new("daiya", "https", "x.example", "").expect("mount"),
    );
    let folder = |uid: &str| {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/folders")
            .header("authorization", "Bearer admin")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "uid": uid, "title": uid }).to_string(),
            ))
            .expect("request");
        req.extensions_mut().insert(jc_router::ClientAddr(
            std::net::Ipv4Addr::new(203, 0, 113, 9).into(),
        ));
        let app = app.clone();
        async move {
            tower::ServiceExt::oneshot(app, req)
                .await
                .expect("answer")
                .status()
        }
    };
    let kinds = || async {
        state
            .store
            .list_audit(org)
            .await
            .expect("rows")
            .into_iter()
            .map(|r| r.kind)
            .collect::<Vec<_>>()
    };

    assert_eq!(folder("ops").await, StatusCode::OK);
    assert_eq!(kinds().await, ["FOLDER_CREATE"]);

    sqlx::raw_sql("ALTER TABLE cr_audit ADD CONSTRAINT refused CHECK (false) NOT VALID")
        .execute(&db.admin)
        .await
        .expect("refuse audit rows");
    let status = folder("trams").await;
    assert!(status.is_server_error(), "{status}");
    let trams = state.store.get_folder(org, "trams").await.expect("read");
    assert!(trams.is_none(), "the folder stands without its row");
    assert_eq!(kinds().await, ["FOLDER_CREATE"]);

    // The request role appends and reads; it never rewrites or removes a row.
    for statement in [
        "UPDATE cr_audit SET actor_id = 'someone'",
        "DELETE FROM cr_audit",
    ] {
        let refused = sqlx::raw_sql(statement).execute(&pool).await;
        assert!(refused.is_err(), "{statement} ran as the request role");
    }
    assert_eq!(kinds().await, ["FOLDER_CREATE"]);

    // A commit the database refuses only at its end (a deferred trigger) answers 500 and the
    // dashboard it would have deleted still stands.
    let send = |method: &'static str, uri: &'static str, body: Option<serde_json::Value>| {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", "Bearer admin")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .expect("request");
        req.extensions_mut().insert(jc_router::ClientAddr(
            std::net::Ipv4Addr::new(203, 0, 113, 9).into(),
        ));
        let app = app.clone();
        async move {
            tower::ServiceExt::oneshot(app, req)
                .await
                .expect("answer")
                .status()
        }
    };
    sqlx::raw_sql("ALTER TABLE cr_audit DROP CONSTRAINT refused")
        .execute(&db.admin)
        .await
        .expect("audit rows again");
    let board = serde_json::json!({
        "dashboard": { "uid": "late", "title": "Late", "schemaVersion": 42, "panels": [] },
        "overwrite": true
    });
    assert_eq!(
        send("POST", "/api/dashboards/db", Some(board)).await,
        StatusCode::OK
    );
    sqlx::raw_sql(
        "CREATE FUNCTION daiya_refuse_late() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$; \
         CREATE CONSTRAINT TRIGGER refused_late AFTER INSERT ON cr_audit \
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION daiya_refuse_late();",
    )
    .execute(&db.admin)
    .await
    .expect("refuse at commit");
    let status = send("DELETE", "/api/dashboards/uid/late", None).await;
    assert!(status.is_server_error(), "{status}");
    let late = state.store.get_dashboard(org, "late").await.expect("read");
    assert!(
        late.is_some(),
        "a delete that did not commit leaves the dashboard"
    );

    drop((app, state));
    pool.close().await;
    db.admin.close().await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .execute(&server)
        .await
        .expect("drop database");
}
