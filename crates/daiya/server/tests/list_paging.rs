#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7524: a count and a page of dashboards read a page of index columns, never the models.
//!
//! `DAIYA_TEST_DATABASE_URL` names an admin connection. Unset: prints `SKIP`.

#[path = "../../../shared/test-support/pg_meter.rs"]
mod pg_meter;

use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::{
    DaiyaStore as _, DashboardRecord, GrafanaRole, InstallationId, InstanceId, OrgId,
    PolicyAuthorizer, RoleBinding, Subject,
};
use daiya_store_pg::{PgStore, connect_test_database};
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use std::sync::Arc;

const DASHBOARDS: u64 = 1000;
const PAGE: u64 = 50;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_count_and_a_page_of_1000_dashboards_read_one_page() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let db = connect_test_database(&url).await.expect("database");
    let (installation, org, instance) = (InstallationId::new(), OrgId::new(), InstanceId::new());
    let now = jc_time::TimestampTz::now();
    db.app
        .upsert_dashboard(
            DashboardRecord {
                installation_id: installation,
                org_id: org,
                instance_id: instance,
                project_id: None,
                uid: "d0".into(),
                folder_uid: None,
                title: "Dashboard 0".into(),
                slug: "dashboard-0".into(),
                tags: vec![],
                panel_count: 1,
                datasource_uids: vec![],
                version: 1,
                schema_version: 42,
                is_provisioned: false,
                provisioned_external_id: None,
                allow_ui_updates: true,
                data: serde_json::json!({ "uid": "d0", "description": "m".repeat(8 * 1024) }),
                created_by: "test".into(),
                updated_by: "test".into(),
                created_at: now,
                updated_at: now,
            },
            "first",
            false,
        )
        .await
        .expect("dashboard");
    // Past the quota of 500 and in one statement: the writes are not what this measures.
    sqlx::query(
        "INSERT INTO cr_dashboard
         SELECT (jsonb_populate_record(d, jsonb_build_object(
             'uid', 'd' || n, 'slug', 'dashboard-' || n, 'title', 'Dashboard ' || n))).*
         FROM cr_dashboard d, generate_series(1, $2::bigint) n
         WHERE d.org_id = $1 AND d.uid = 'd0'",
    )
    .bind(org.as_uuid())
    .bind((DASHBOARDS - 1) as i64)
    .execute(&db.admin)
    .await
    .expect("copies");

    let meter = pg_meter::PgMeter::start(&db.app_config.host, db.app_config.port).await;
    let mut metered = db.app_config.clone();
    metered.host = "127.0.0.1".into();
    metered.port = meter.port;
    let store = Arc::new(PgStore::new(metered.connect().await.expect("metered pool")));
    let state = DaiyaState::new(
        installation,
        org,
        instance,
        store,
        Arc::new(PolicyAuthorizer::new(vec![RoleBinding::from_grafana_role(
            org,
            "viewer",
            GrafanaRole::Viewer,
            None,
        )])),
    )
    .with_tokens(vec![TokenBinding::new(
        "viewer",
        Subject::authenticated("viewer", vec![org], vec![]),
    )]);
    let app = api_router(
        state,
        jc_router::Mount::new("daiya", "https", "x.example", "").expect("mount"),
    );
    let get = async |path: String| {
        let request = Request::builder()
            .uri(path)
            .header("authorization", "Bearer viewer")
            .body(Body::empty())
            .expect("request");
        let answer = tower::ServiceExt::oneshot(app.clone(), request)
            .await
            .expect("answer");
        assert_eq!(answer.status(), StatusCode::OK);
        answer.into_body().collect().await.expect("body").to_bytes()
    };

    let mut measured = Vec::new();
    for path in [
        "/odata/v4/Dashboards/$count".to_owned(),
        format!("/odata/v4/Dashboards?$top={PAGE}"),
    ] {
        meter.take();
        let body = get(path.clone()).await;
        if path.ends_with("$count") {
            assert_eq!(body.as_ref(), DASHBOARDS.to_string().as_bytes());
        } else {
            let body: serde_json::Value = serde_json::from_slice(&body).expect("page");
            assert_eq!(body["value"].as_array().expect("rows").len() as u64, PAGE);
        }
        let (rows, bytes) = meter.take();
        println!("{DASHBOARDS} dashboards: {path} reads {rows} rows, {bytes} bytes");
        measured.push((path, rows, bytes));
    }
    sqlx::query("DELETE FROM cr_dashboard WHERE org_id = $1")
        .bind(org.as_uuid())
        .execute(&db.admin)
        .await
        .expect("clean");
    for (path, rows, bytes) in measured {
        assert!(rows <= 2 * PAGE, "{path} reads {rows} rows");
        assert!(bytes < 128 * 1024, "{path} reads {bytes} bytes");
    }
}
