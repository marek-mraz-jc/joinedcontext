//! Single-person approvals on a database: with the mode on, another person holding a role in the
//! Organization refuses the approval, and the sole holder approves after a fresh second factor
//! (T-7823).

mod common;

use joinedcontext_portal::auth::session::now_unix;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::single_person::{approves_alone, set};
use joinedcontext_portal::state::AppState;

async fn world() -> (AppState, String, String) {
    let state = common::on_database(AppState::new(Config::for_tests(), None)).await;
    let pool = state.db.clone().expect("a database");
    let suffix = common::unique::unique();
    let organization = format!("sp-{suffix}");
    let project = format!("spp-{suffix}");
    common::keyed_project(&state, &pool, &project, &organization).await;
    let mut conn = pool.acquire().await.expect("a connection");
    set(&mut conn, &organization, true, "test", None)
        .await
        .expect("the mode is on");
    (state, organization, project)
}

#[tokio::test]
async fn another_person_holding_a_role_refuses_the_approval_naming_the_count() {
    let (state, organization, project) = world().await;
    common::bind(&state.mirror, "ann", &project);
    let refused = approves_alone(
        &state,
        &common::person("bob"),
        &project,
        true,
        Some(now_unix()),
    )
    .await
    .expect_err("ann holds a role there");
    assert!(
        matches!(&refused, ApiError::SelfApproval(m)
            if m.contains(&organization) && m.contains("1 other people")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_one_person_with_a_role_approves_after_a_fresh_second_factor_and_not_without() {
    let (state, _, project) = world().await;
    common::bind(&state.mirror, "ann", &project);
    let ann = common::person("ann");
    assert!(
        approves_alone(&state, &ann, &project, true, Some(now_unix()))
            .await
            .expect("answered")
    );
    let stale = approves_alone(&state, &ann, &project, true, Some(now_unix() - 86_400)).await;
    assert!(
        matches!(stale, Err(ApiError::SecondFactorRequired)),
        "{stale:?}"
    );
    let none = approves_alone(&state, &ann, &project, true, None).await;
    assert!(
        matches!(none, Err(ApiError::SecondFactorRequired)),
        "{none:?}"
    );
    assert!(
        !approves_alone(&state, &ann, &project, false, Some(now_unix()))
            .await
            .expect("an agent is not let through")
    );
}

#[tokio::test]
async fn a_project_no_organization_holds_is_not_approved_alone() {
    let (state, _, _) = world().await;
    let unknown = format!("nowhere-{}", common::unique::unique());
    assert!(!approves_alone(
        &state,
        &common::person("ann"),
        &unknown,
        true,
        Some(now_unix())
    )
    .await
    .expect("answered"));
}
