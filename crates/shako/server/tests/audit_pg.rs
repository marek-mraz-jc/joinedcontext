#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! S6-20 (T-7571) against a live PostgreSQL: a forge write and its audit row commit in one
//! transaction through the door, so a refused audit row leaves no change, and the request role
//! can only append rows. One test: migrations of two databases at once race on the shared roles.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection. Unset: prints `SKIP`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, Principal,
    RoleBinding, ShakoSigningKey, StaticTokenBinding, Store as _, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_pg::PgStore;
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::str::FromStr as _;
use std::sync::Arc;

const INSTANCE: InstanceId = InstanceId::from_u128(1);
const CITY: OrgId = OrgId::from_u128(10);
const R: &str = "/odata/Repositories(Owner='city',Name='bikes')";

struct Forge {
    server: PgPool,
    admin: PgPool,
    name: String,
    _repositories: tempfile::TempDir,
    store: Arc<PgStore>,
    app: Router,
}

impl Forge {
    async fn start() -> Option<Self> {
        let Ok(url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
            println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
            return None;
        };
        let options = PgConnectOptions::from_str(&url).expect("valid SHAKO_TEST_DATABASE_URL");
        let server = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");
        let name = format!("shako_audit_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&server)
            .await
            .expect("create");
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone().database(&name))
            .await
            .expect("connect");
        PgStore::run_migrations(&admin).await.expect("migrate");
        let password = uuid::Uuid::new_v4().simple().to_string();
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE ROLE {name} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{password}';
             GRANT shako_app TO {name};"
        )))
        .execute(&admin)
        .await
        .expect("login");
        let store = Arc::new(
            PgStore::connect_with(
                options.database(&name).username(&name).password(&password),
                4,
            )
            .await
            .expect("store"),
        );
        let repositories = tempfile::tempdir().expect("tempdir");
        let state = ShakoState::new(
            INSTANCE,
            repositories.path().to_path_buf(),
            store.clone(),
            Arc::new(LocalAuthorizer::new(vec![RoleBinding {
                org_id: CITY,
                principal: Principal::User("city".into()),
                actions: vec![Action::Admin, Action::Write],
                project_id: None,
                repo: None,
            }])),
            TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
                StaticTokenBinding::new(
                    "city-token",
                    Subject::authenticated("city", "city", vec![CITY]),
                ),
            ]),
            OutboundGuard::new(OutboundPolicy::default()),
        );
        let mount = jc_router::Mount::new("shako", "https", "city.example", "/git").expect("mount");
        let app = router(state, mount);
        Some(Self {
            server,
            admin,
            name,
            _repositories: repositories,
            store,
            app,
        })
    }

    async fn post(&self, uri: &str, body: Value) -> StatusCode {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", "Bearer city-token")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        tower::ServiceExt::oneshot(self.app.clone(), request)
            .await
            .expect("answer")
            .status()
    }

    async fn audit_kinds(&self) -> Vec<String> {
        let rows = self.store.list_audit(INSTANCE, CITY).await.expect("rows");
        rows.into_iter().map(|r| r.kind).collect()
    }

    async fn stop(self) {
        drop(self.app);
        self.store.pool().close().await;
        self.admin.close().await;
        let name = &self.name;
        sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
            .execute(&self.server)
            .await
            .expect("drop database");
        sqlx::raw_sql(AssertSqlSafe(format!("DROP ROLE {name}")))
            .execute(&self.server)
            .await
            .expect("drop role");
    }
}

#[tokio::test]
async fn a_forge_write_commits_with_its_audit_row_or_neither_stands() {
    let Some(forge) = Forge::start().await else {
        return;
    };
    let create = |name: &str| json!({"Owner": "city", "Name": name});
    assert_eq!(
        forge.post("/odata/CreateRepository", create("bikes")).await,
        StatusCode::OK
    );
    assert_eq!(forge.audit_kinds().await, ["REPO_CREATE"]);

    // From here every audit row is refused: the writes that would carry one must not stand.
    sqlx::raw_sql("ALTER TABLE shako.audit ADD CONSTRAINT refused CHECK (false) NOT VALID")
        .execute(&forge.admin)
        .await
        .expect("refuse audit rows");
    let status = forge.post("/odata/CreateRepository", create("trams")).await;
    assert!(status.is_server_error(), "{status}");
    let trams = forge
        .store
        .get_repo_by_name(INSTANCE, CITY, "city", "trams")
        .await
        .expect("read");
    assert!(trams.is_none(), "the repository stands without its row");
    let status = forge
        .post(
            &format!("{R}/Shako.Protect"),
            json!({"Rule": {"rule_name": "main"}}),
        )
        .await;
    assert!(status.is_server_error(), "{status}");
    let bikes = forge
        .store
        .get_repo_by_name(INSTANCE, CITY, "city", "bikes")
        .await
        .expect("read")
        .expect("bikes");
    let rule = forge
        .store
        .get_protection_rule(INSTANCE, CITY, bikes.id, "main")
        .await
        .expect("read");
    assert!(rule.is_none(), "the rule stands without its row");
    assert_eq!(forge.audit_kinds().await, ["REPO_CREATE"]);

    // The request role appends and reads; it never rewrites or removes a row.
    for statement in [
        "UPDATE shako.audit SET actor_id = 'someone'",
        "DELETE FROM shako.audit",
    ] {
        let refused = sqlx::raw_sql(statement).execute(forge.store.pool()).await;
        assert!(refused.is_err(), "{statement} ran as the request role");
    }
    assert_eq!(forge.audit_kinds().await, ["REPO_CREATE"]);
    forge.stop().await;
}
