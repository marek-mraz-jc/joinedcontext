//! Branch projects (ARCH/TEN-12, CHG-15, RTE-05; T-7213): the door that opens and renews a branch,
//! and a branch's name, which resolves only on the branch's own id address, so a name in a body,
//! a tool argument or another project's address answers like an unknown project.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it).
//! A missing variable fails, never skips. Each test uses names of its own run.

mod common;

use axum::http::StatusCode;
use jc_types::ProjectId;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::project_ids;
use joinedcontext_portal::request_id::placed;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{envelope, keyed_project, person, send, send_placed};

async fn state() -> (AppState, sqlx::PgPool) {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    (state, pool)
}

fn run_name(test: &str) -> String {
    format!("{test}-{}", ProjectId::mint().path_segment())
}

/// `who@hel.fi` holds `verbs` on Apps and Endpoints in `project`.
fn grant(state: &AppState, who: &str, project: &str, verbs: &[&str]) {
    let role = format!("{who}-{project}-role");
    state.mirror.upsert(envelope(
        "Role",
        &role,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App", "Endpoint"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        &format!("{who}-in-{project}"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role,
                "scope": { "project": project } }),
    ));
}

/// A project of its own Organization, with `ana` proposing in it.
async fn parent(state: &AppState, pool: &sqlx::PgPool, test: &str) -> String {
    let project = run_name(test);
    keyed_project(state, pool, &project, &run_name("brorg")).await;
    grant(state, "ana", &project, &["read", "propose"]);
    project
}

#[tokio::test]
async fn a_branch_name_resolves_only_on_its_own_id_address() {
    let (state, pool) = state().await;
    let project = parent(&state, &pool, "brresolve").await;
    let parent_key = project_ids::key_of(&pool, &state.mirror, &project)
        .await
        .expect("key")
        .expect("a keyed parent");
    let branch = project_ids::open_branch(&pool, parent_key, "sub-ana", 7, 5)
        .await
        .expect("open")
        .expect("a branch");
    let own = branch.id.path_segment();
    let other = ProjectId::mint().path_segment();
    let key_of = |name: String, at: Option<String>| {
        let (pool, mirror) = (pool.clone(), state.mirror.clone());
        async move {
            placed(at, project_ids::key_of(&pool, &mirror, &name))
                .await
                .expect("key_of")
        }
    };
    // A name in a body or a tool argument, on a name address: no branch.
    assert_eq!(key_of(branch.name.clone(), None).await, None);
    // Another project's id address naming the branch: no branch either.
    assert_eq!(key_of(branch.name.clone(), Some(other.clone())).await, None);
    let key = key_of(branch.name.clone(), Some(own.clone()))
        .await
        .expect("the branch on its own id address");
    assert_eq!(key.project, branch.id);
    assert_eq!(
        key.org, parent_key.org,
        "a branch stays in its parent's Organization"
    );
    // On an id address every pair present agrees (RTE-05): the parent's name there is unknown.
    assert_eq!(key_of(project.clone(), Some(own)).await, None);
    assert_eq!(key_of(project.clone(), Some(other)).await, None);
    assert_eq!(
        key_of(project.clone(), Some(parent_key.project.path_segment())).await,
        Some(parent_key)
    );
    assert_eq!(key_of(project, None).await, Some(parent_key));
}

#[tokio::test]
async fn a_branch_is_limited_per_parent_renewed_to_30_days_and_audited() {
    let (state, pool) = state().await;
    let project = parent(&state, &pool, "brlimit").await;
    let parent_key = project_ids::key_of(&pool, &state.mirror, &project)
        .await
        .expect("key")
        .expect("a keyed parent");
    let branch = project_ids::open_branch(&pool, parent_key, "sub-ana", 7, 1)
        .await
        .expect("open")
        .expect("the first branch");
    assert_eq!(
        project_ids::open_branch(&pool, parent_key, "sub-ana", 7, 1)
            .await
            .expect("open"),
        Err(project_ids::NoBranch::AtLimit),
        "one live branch per project is the limit here"
    );
    let mut expiries = vec![branch.expires_at];
    while let Some(expires_at) = project_ids::renew_branch(&pool, parent_key, branch.id, "sub-ana")
        .await
        .expect("renew")
    {
        expiries.push(expires_at);
    }
    let days: Vec<i64> = expiries
        .windows(2)
        .map(|w| (w[1] - w[0] + 43_200) / 86_400)
        .collect();
    assert_eq!(days, [7, 7, 7, 2], "7 days at a time, never past 30 days");
    let (audited,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM project_branch_renewals WHERE branch = $1::uuid AND renewed_by = 'sub-ana'",
    )
    .bind(branch.id.to_string())
    .fetch_one(&pool)
    .await
    .expect("renewals");
    assert_eq!(audited, 4, "each renewal is audited");
    sqlx::query("DELETE FROM project_branch_renewals WHERE branch = $1::uuid")
        .bind(branch.id.to_string())
        .execute(&pool)
        .await
        .expect_err("an audit row stays");
    // A branch is no parent, and neither is an Organization's reserved project (TEN-11).
    let branch_key = placed(
        Some(branch.id.path_segment()),
        project_ids::key_of(&pool, &state.mirror, &branch.name),
    )
    .await
    .expect("key")
    .expect("the branch's key");
    let org: String =
        sqlx::query_scalar("SELECT organization FROM project_ids WHERE id = $1::uuid")
            .bind(parent_key.project.to_string())
            .fetch_one(&pool)
            .await
            .expect("organization");
    let reserved = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("the reserved project");
    for key in [branch_key, reserved] {
        assert_eq!(
            project_ids::open_branch(&pool, key, "sub-ana", 7, 5)
                .await
                .expect("open"),
            Err(project_ids::NoBranch::NotAParent)
        );
    }
}

#[tokio::test]
async fn the_branch_door_opens_for_a_proposer_and_answers_others_like_an_unknown_project() {
    let (state, pool) = state().await;
    let project = parent(&state, &pool, "brdoor").await;
    grant(&state, "ben", &project, &["read"]);
    let unknown = send(
        &state,
        person("ana"),
        "POST",
        "/api/v1/projects/no-such-project/branches",
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    let reader = send(
        &state,
        person("ben"),
        "POST",
        &format!("/api/v1/projects/{project}/branches"),
        None,
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    let stranger = send(
        &state,
        person("eve"),
        "POST",
        &format!("/api/v1/projects/{project}/branches"),
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);

    let opened = send(
        &state,
        person("ana"),
        "POST",
        &format!("/api/v1/projects/{project}/branches"),
        None,
    )
    .await;
    assert_eq!(opened.status, StatusCode::CREATED, "{}", opened.text);
    let branch: Value = serde_json::from_str(&opened.text).expect("json");
    let id = branch["id"].as_str().expect("id").to_owned();
    assert_eq!(branch["name"], format!("b-{id}"));
    assert_eq!(branch["parent"], project.as_str());

    // A branch's name never stands for a project on a name address, the door's included.
    let named = send(
        &state,
        person("ana"),
        "POST",
        &format!("/api/v1/projects/b-{id}/branches"),
        None,
    )
    .await;
    assert_eq!(named.status, StatusCode::NOT_FOUND, "{}", named.text);
    // The parent's name on the branch's id address disagrees with it: unknown, not the parent.
    let disagreeing = send_placed(
        &state,
        person("ana"),
        "POST",
        &format!("/api/v1/projects/{project}/branches"),
        &id,
    )
    .await;
    assert_eq!(
        disagreeing.status,
        StatusCode::NOT_FOUND,
        "{}",
        disagreeing.text
    );

    let renewal = format!("/api/v1/projects/{project}/branches/{id}/renewal");
    let refused = send(&state, person("ben"), "POST", &renewal, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    let renewed = send(&state, person("ana"), "POST", &renewal, None).await;
    assert_eq!(renewed.status, StatusCode::OK, "{}", renewed.text);
    let renewed: Value = serde_json::from_str(&renewed.text).expect("json");
    assert!(
        renewed["expiresAt"].as_str() > branch["expiresAt"].as_str(),
        "{renewed} {branch}"
    );
    let missing = send(
        &state,
        person("ana"),
        "POST",
        &format!(
            "/api/v1/projects/{project}/branches/{}/renewal",
            ProjectId::mint().path_segment()
        ),
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);
}
