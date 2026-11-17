//! The project id registry (ARCH/TEN-01, TEN-04, TEN-12; T-7190, migration 0066): every project
//! the project list shows carries an id minted once, stable across calls, different for two
//! projects, never renamed, released or reused, and a branch with its parent's name never
//! answers for the parent.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it).
//! A missing variable fails, never skips. Each test uses names of its own run.

mod common;

use axum::http::StatusCode;
use jc_types::ProjectId;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::project_ids;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{bind, envelope, person, send};

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn run_name(test: &str) -> String {
    format!("{test}-{}", ProjectId::mint().path_segment())
}

#[tokio::test]
async fn ids_are_minted_once_and_differ_per_project() {
    let pool = pool().await;
    let a = run_name("ids-a");
    let b = run_name("ids-b");
    let first = project_ids::ensure(&pool, &[a.clone(), b.clone()])
        .await
        .expect("ensure");
    let again = project_ids::ensure(&pool, &[b.clone(), a.clone()])
        .await
        .expect("ensure again");
    assert_eq!(first.len(), 2);
    assert_eq!(first, again, "an id is minted once and read back after");
    assert_ne!(first[&a], first[&b], "two projects never share an id");
}

#[tokio::test]
async fn an_id_is_never_renamed_released_or_reused() {
    let pool = pool().await;
    let name = run_name("keep");
    let id = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure")[&name];
    for statement in [
        "DELETE FROM project_ids WHERE id = $1::uuid",
        "UPDATE project_ids SET name = name || '-renamed' WHERE id = $1::uuid",
        "UPDATE project_ids SET id = gen_random_uuid() WHERE id = $1::uuid",
    ] {
        let refused = sqlx::query(statement)
            .bind(id.to_string())
            .execute(&pool)
            .await
            .expect_err(statement);
        assert!(
            refused
                .to_string()
                .contains("never renamed, released or reused"),
            "{statement}: {refused}"
        );
    }
    let still = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure");
    assert_eq!(still[&name], id);
}

#[tokio::test]
async fn a_branch_with_its_parents_name_never_answers_for_the_parent() {
    let pool = pool().await;
    let name = run_name("branch");
    let parent = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure")[&name];
    let branch = ProjectId::mint();
    sqlx::query("INSERT INTO project_ids (id, name, parent) VALUES ($1::uuid, $2, $3::uuid)")
        .bind(branch.to_string())
        .bind(&name)
        .bind(parent.to_string())
        .execute(&pool)
        .await
        .expect("a branch may hold its parent's name");
    let live = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure");
    assert_eq!(live[&name], parent);
    assert_ne!(live[&name], branch);
}

#[tokio::test]
async fn the_project_list_shows_each_readable_projects_id() {
    let pool = pool().await;
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    let project = run_name("listed");
    let hidden = run_name("hidden");
    for name in [&project, &hidden] {
        state.mirror.upsert(envelope(
            "Project",
            name,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({}),
        ));
    }
    bind(&state.mirror, "ada", &project);
    let answer = send(&state, person("ada"), "GET", "/api/v1/projects", None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    let items = body["items"].as_array().cloned().unwrap_or_default();
    let listed: Vec<&Value> = items
        .iter()
        .filter(|item| item["name"] == project.as_str())
        .collect();
    assert_eq!(listed.len(), 1, "{body}");
    let id = listed[0]["id"].as_str().expect("an id");
    let expected = project_ids::ensure(&pool, std::slice::from_ref(&project))
        .await
        .expect("ensure")[&project];
    assert_eq!(ProjectId::from_path_segment(id), Ok(expected));
    assert!(
        items.iter().all(|item| item["name"] != hidden.as_str()),
        "a project the caller cannot read is not listed: {body}"
    );
}
