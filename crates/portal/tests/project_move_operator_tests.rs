//! Moving a project to another Organization (T-7305): an operator `ProjectMove` Change, approved
//! by a second operator, re-keys the project's registry row and every row keyed by it in one
//! transaction. Afterwards the rows read only under the new Organization's key (forced RLS), the
//! old Organization keys the project no more, a second move is a no-op, the plan counts rows per
//! table, and nobody but an operator with a second factor reaches the door.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it). A
//! missing variable fails, never skips. Each test uses Organizations and projects of its own run.

mod common;

use axum::http::StatusCode;
use jc_types::ProjectKey;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{envelope, person, send};

const CHANGES: &str = "/api/v1/operator/changes";

/// A Portal serving as a role apart from the migrating one, without BYPASSRLS, as on the cluster
/// (T-7179): forced row-level security holds for every statement of these tests.
async fn world() -> AppState {
    let owner_url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let owner = joinedcontext_portal::db::connect(&owner_url)
        .await
        .expect("connect + migrate");
    let role = format!("move{}", common::unique::unique());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE \"{role}\" LOGIN PASSWORD 'serving-pw'"
    )))
    .execute(&owner)
    .await
    .expect("a serving role");
    let mut serving_url = url::Url::parse(&owner_url).expect("the test URL");
    serving_url.set_username(&role).expect("a user");
    serving_url
        .set_password(Some("serving-pw"))
        .expect("a password");
    // The grants to a serving role race when two tests of this binary boot at once.
    static BOOT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let booting = BOOT.lock().await;
    let pool = joinedcontext_portal::db::connect_as(serving_url.as_str(), Some(&owner_url))
        .await
        .expect("serve as the serving role");
    drop(booting);
    let mut config = Config::for_tests();
    config.decisions_need_second_factor = false;
    AppState::new(config, None).with_db(pool)
}

fn pool(state: &AppState) -> &sqlx::PgPool {
    state.db.as_ref().expect("db")
}

fn operator(state: &AppState, name: &str) -> Identity {
    let mut who = person(name);
    who.groups = vec![state.config.bootstrap_admins.clone()];
    who.second_factor = true;
    who
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_default()
}

fn run(test: &str) -> String {
    format!("{test}-{}", common::unique::unique())
}

/// `project` filed under Organization `from` in the mirror and the registry, with one draft and
/// one activity row under its key; both Organizations active.
async fn placed(state: &AppState, project: &str, from: &str, to: &str) -> ProjectKey {
    for organization in [from, to] {
        sqlx::query(
            "INSERT INTO organizations (name, host, state, created_by) \
             VALUES ($1, $1 || '.example', 'active', 'test') ON CONFLICT DO NOTHING",
        )
        .bind(organization)
        .execute(pool(state))
        .await
        .expect("an Organization");
    }
    manifest(state, project, from);
    let key = joinedcontext_portal::project_ids::key(pool(state), from, project)
        .await
        .expect("registry")
        .expect("a key");
    let mut tx = joinedcontext_portal::db::tenant_tx(pool(state), key)
        .await
        .expect("keyed");
    sqlx::query(
        "INSERT INTO drafts (project, kind, name, manifest, touched_by, touched_kind) \
         VALUES ($1, 'Space', 'air', '{}', 'test', 'person')",
    )
    .bind(project)
    .execute(&mut *tx)
    .await
    .expect("a draft");
    sqlx::query(
        "INSERT INTO activity (time, project, kind, source, summary, severity) \
         VALUES (now(), $1, 'Space', 'test', 'air created', 'info')",
    )
    .bind(project)
    .execute(&mut *tx)
    .await
    .expect("an activity row");
    tx.commit().await.expect("commit");
    key
}

fn manifest(state: &AppState, project: &str, organization: &str) {
    state.mirror.upsert(envelope(
        "Project",
        project,
        ORG_NAMESPACE,
        json!({ "organizationRef": organization }),
    ));
}

/// The drafts a transaction keyed with `key` reads of `project`.
async fn drafts_under(state: &AppState, key: ProjectKey, project: &str) -> i64 {
    let mut tx = joinedcontext_portal::db::tenant_tx(pool(state), key)
        .await
        .expect("keyed");
    sqlx::query_scalar("SELECT count(*) FROM drafts WHERE project = $1")
        .bind(project)
        .fetch_one(&mut *tx)
        .await
        .expect("drafts")
}

async fn propose(state: &AppState, who: Identity, project: &str, to: &str) -> (StatusCode, Value) {
    let change = json!({ "kind": "ProjectMove", "project": project, "organization": to });
    let answer = send(state, who, "POST", CHANGES, Some(change)).await;
    (answer.status, body(&answer.text))
}

async fn approve(state: &AppState, who: Identity, id: &str) -> (StatusCode, Value) {
    let answer = send(state, who, "POST", &format!("{CHANGES}/{id}/approve"), None).await;
    (answer.status, body(&answer.text))
}

async fn plan(state: &AppState, who: Identity, project: &str, to: &str) -> (StatusCode, Value) {
    let path = format!("/api/v1/operator/project-moves/plan?project={project}&organization={to}");
    let answer = send(state, who, "GET", &path, None).await;
    (answer.status, body(&answer.text))
}

/// The move itself: refused while the manifest names the old Organization, planned with counts,
/// applied by a second operator; then the rows read under the new key only, the old Organization
/// keys the project no more, and the audit holds counts, never a row.
#[tokio::test]
async fn a_moved_project_reads_only_under_its_new_organization() {
    let state = world().await;
    let from = run("hel");
    let to = run("bbsk");
    let project = run("moved");
    let old = placed(&state, &project, &from, &to).await;
    assert_eq!(drafts_under(&state, old, &project).await, 1);

    // The manifest still names the old Organization: the move would key the project where the
    // mirror does not place it.
    let (status, refused) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused.to_string().contains("spec.organizationRef"),
        "{refused}"
    );

    manifest(&state, &project, &to);
    let (status, planned) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    assert_eq!(planned["from"], json!(from));
    assert_eq!(planned["blockers"], json!([]));
    assert_eq!(planned["done"], json!(false));
    let tables = planned["tables"].as_array().expect("tables");
    assert!(
        tables.contains(&json!({ "table": "drafts", "rows": 1 }))
            && tables.contains(&json!({ "table": "activity", "rows": 1 })),
        "{planned}"
    );

    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, self_approved) = approve(&state, operator(&state, "ops-a"), &id).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{self_approved}");
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["state"], json!("applied"));

    assert_eq!(
        drafts_under(&state, old, &project).await,
        0,
        "the old Organization's key reads nothing"
    );
    let new = joinedcontext_portal::project_ids::recorded(pool(&state), &project)
        .await
        .expect("registry")
        .expect("a key");
    assert_eq!(new.project, old.project, "the project keeps its id");
    assert_ne!(new.org, old.org);
    assert_eq!(drafts_under(&state, new, &project).await, 1);
    assert_eq!(
        joinedcontext_portal::project_ids::key(pool(&state), &from, &project)
            .await
            .expect("registry"),
        None,
        "the old Organization keys the project no more"
    );
    assert_eq!(
        joinedcontext_portal::project_ids::key_of(pool(&state), &state.mirror, &project)
            .await
            .expect("registry"),
        Some(new),
        "a request finds the project under its new key"
    );

    let after: Value = sqlx::query_scalar(
        "SELECT after FROM operator_audit WHERE change_id = $1::uuid AND action = 'applied ProjectMove'",
    )
    .bind(&id)
    .fetch_one(pool(&state))
    .await
    .expect("the audit row");
    assert_eq!(after["rows"], json!(2), "{after}");
    assert!(
        !after.to_string().contains("air"),
        "no row content: {after}"
    );

    // A second move to the same Organization changes nothing.
    let (status, again) = plan(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["done"], json!(true));
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &to).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(drafts_under(&state, new, &project).await, 1);
}

/// A project named like the Organization it moves to (jc-dev's bbsk into bbsk) moves: the
/// Organization's reserved project never answers for a project's name (TEN-11).
#[tokio::test]
async fn a_project_named_like_its_new_organization_moves() {
    let state = world().await;
    let from = run("hel");
    let project = run("bbsk");
    let old = placed(&state, &project, &from, &project).await;
    manifest(&state, &project, &project);
    let (status, made) = propose(&state, operator(&state, "ops-a"), &project, &project).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, applied) = approve(&state, operator(&state, "ops-b"), &id).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let new = joinedcontext_portal::project_ids::recorded(pool(&state), &project)
        .await
        .expect("registry")
        .expect("a key");
    assert_eq!(drafts_under(&state, new, &project).await, 1);
    assert_eq!(drafts_under(&state, old, &project).await, 0);
    let reserved = joinedcontext_portal::project_ids::reserved(pool(&state), &project)
        .await
        .expect("registry")
        .expect("the reserved project");
    assert_ne!(reserved.project, new.project);
}

/// Only an operator with a second factor plans or asks for a move; the registry refuses a moved
/// Organization outside an approved move, and the keyed tables stay forced.
#[tokio::test]
async fn only_an_operator_moves_and_nothing_else_moves_a_project() {
    let state = world().await;
    let from = run("hel");
    let to = run("ba");
    let project = run("guarded");
    placed(&state, &project, &from, &to).await;
    manifest(&state, &project, &to);

    let mut password_only = operator(&state, "ops-a");
    password_only.second_factor = false;
    let administrator = common::organization_admin(&state.mirror, "org-admin");
    for who in [person("someone"), password_only, administrator] {
        let (status, refused) = propose(&state, who.clone(), &project, &to).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
        let (status, refused) = plan(&state, who, &project, &to).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    }
    let (status, refused) = propose(&state, operator(&state, "ops-a"), "Not A Name", &to).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, refused) = propose(&state, operator(&state, "ops-a"), &project, "nowhere").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("organization:"), "{refused}");
    let (status, refused) = plan(&state, operator(&state, "ops-a"), "no-such-project", &to).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");

    let moved = sqlx::query(
        "UPDATE project_ids SET organization = $2 WHERE name = $1 AND parent IS NULL AND NOT reserved",
    )
    .bind(&project)
    .bind(&to)
    .execute(pool(&state))
    .await;
    assert!(
        moved.is_err(),
        "the registry refuses a move outside a ProjectMove"
    );

    // The tables stay forced: the move lifts FORCE only inside its own transaction.
    let unforced: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class WHERE relname IN ('drafts', 'activity') AND NOT relforcerowsecurity",
    )
    .fetch_all(pool(&state))
    .await
    .expect("catalog");
    assert_eq!(unforced, Vec::<String>::new());
}
