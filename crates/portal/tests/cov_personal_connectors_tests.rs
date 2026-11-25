//! The move of old personal connectors: what stays aside and why (T-7823).
//!
//! Runs against `JC_PORTAL_TEST_DATABASE_URL` (a migrated-on-connect PostgreSQL).

use serde_json::{json, Value};

use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::personal_connectors::move_one;
use joinedcontext_portal::state::AppState;

mod common;
use common::unique::unique;

/// A Portal whose people's rows live under an Organization of its own, with one public Endpoint
/// in project `mesto` that every reader of that project may read.
async fn world(mirror_holds_endpoints: bool) -> (AppState, String) {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let persons = format!("p{}", unique());
    let seed = joinedcontext_portal::host_registry::SeedOrganization {
        name: persons.clone(),
        organization_id: None,
        host: format!("portal.{persons}.example"),
        catalogue_id: None,
    };
    let mut config = Config::for_tests();
    config.installation_organization = Some(seed.clone());
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    joinedcontext_portal::host_registry::seed_organizations(&pool, &[seed])
        .await
        .expect("the installation Organization");
    let state = AppState::new(config, None).with_db(pool);
    if mirror_holds_endpoints {
        for (name, project) in [("air", "mesto"), ("secret", "tajne")] {
            state.mirror.upsert(common::envelope(
                "Endpoint",
                name,
                project,
                json!({ "contextSpaceRef": project, "slug": name.repeat(26)[..26].to_owned(),
                        "audience": "organization" }),
            ));
        }
        state.mirror.upsert(common::envelope(
            "Role",
            "endpoint-readers",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": ["read"] }] }),
        ));
    }
    (state, persons)
}

fn reads_mesto(state: &AppState, who: &str) {
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{who}-reads-mesto"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": "endpoint-readers",
                "scope": { "project": "mesto" } }),
    ));
}

async fn keep(state: &AppState, persons: &str, subject: &str, doc: &Value) {
    let pool = state.db.as_ref().expect("a pool");
    let mut tx = joinedcontext_portal::project_ids::person_tx(pool, &state.mirror, Some(persons))
        .await
        .expect("the key")
        .expect("a keyed transaction");
    joinedcontext_portal::db::save_preferences(&mut tx, subject, doc)
        .await
        .expect("saved");
    tx.commit().await.expect("committed");
}

async fn stored(state: &AppState, persons: &str, subject: &str) -> Value {
    let pool = state.db.as_ref().expect("a pool");
    let mut tx = joinedcontext_portal::project_ids::person_tx(pool, &state.mirror, Some(persons))
        .await
        .expect("the key")
        .expect("a keyed transaction");
    joinedcontext_portal::db::load_preferences(&mut tx, subject)
        .await
        .expect("loaded")
        .expect("a row")
}

fn in_group(who: &str, group: &str) -> Identity {
    Identity {
        groups: vec![group.to_owned()],
        ..common::person(who)
    }
}

fn client_policy(state: &AppState, group: &str, above_ceiling: &str) {
    state.mirror.upsert(common::envelope(
        "DelegationPolicy",
        &format!("policy-{group}"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "kind": "Group", "name": group }], "projects": ["mesto"],
                "allowedClients": ["claude"], "aboveCeiling": above_ceiling }),
    ));
}

#[tokio::test]
async fn a_move_before_the_first_mirror_asks_to_try_again() {
    let (state, persons) = world(false).await;
    let who = format!("a{}", unique());
    keep(
        &state,
        &persons,
        &format!("sub-{who}"),
        &json!({ "connectors": [{ "name": "fav", "select": {} }] }),
    )
    .await;
    let said = move_one(&state, &common::person(&who))
        .await
        .expect_err("no mirror yet");
    assert!(said.to_string().contains("try again in a minute"), "{said}");
    let row = stored(&state, &persons, &format!("sub-{who}")).await;
    assert!(row.get("connectors").is_some(), "nothing was lost: {row}");
}

#[tokio::test]
async fn a_client_token_and_a_person_without_a_row_move_nothing() {
    let (state, persons) = world(true).await;
    let who = format!("a{}", unique());
    let moved = move_one(&state, &common::person(&who))
        .await
        .expect("no row");
    assert_eq!(
        moved.moved + moved.present + moved.empty + moved.unreadable,
        0
    );
    keep(
        &state,
        &persons,
        &format!("sub-{who}"),
        &json!({ "connectors": [{ "name": "fav", "select": {} }] }),
    )
    .await;
    let client = Identity {
        client: Some("some-app".to_owned()),
        ..common::person(&who)
    };
    let moved = move_one(&state, &client).await.expect("a client");
    assert_eq!(moved.moved, 0);
    assert!(stored(&state, &persons, &format!("sub-{who}"))
        .await
        .get("connectors")
        .is_some());
}

#[tokio::test]
async fn a_row_without_old_connectors_is_left_alone() {
    let (state, persons) = world(true).await;
    let who = format!("a{}", unique());
    keep(
        &state,
        &persons,
        &format!("sub-{who}"),
        &json!({ "theme": "dark" }),
    )
    .await;
    let moved = move_one(&state, &common::person(&who))
        .await
        .expect("no list");
    assert_eq!(moved.moved, 0);
    assert_eq!(
        stored(&state, &persons, &format!("sub-{who}")).await,
        json!({ "theme": "dark" })
    );
}

#[tokio::test]
async fn an_old_connector_above_a_queueing_ceiling_stays_aside_with_its_line() {
    let (state, persons) = world(true).await;
    let who = format!("a{}", unique());
    let group = format!("g{}", unique());
    reads_mesto(&state, &who);
    client_policy(&state, &group, "queue");
    keep(
        &state,
        &persons,
        &format!("sub-{who}"),
        &json!({ "connectors": [{ "name": "fav", "select": { "projects": ["mesto"] } }] }),
    )
    .await;
    let moved = move_one(&state, &in_group(&who, &group))
        .await
        .expect("a move");
    assert_eq!((moved.moved, moved.empty), (0, 1), "{moved:?}");
    let row = stored(&state, &persons, &format!("sub-{who}")).await;
    assert!(row.get("connectors").is_none(), "{row}");
    let aside = row["connectorsNotMoved"].as_array().expect("kept aside");
    assert_eq!(aside.len(), 1, "{row}");
    assert_eq!(aside[0]["name"], "fav");
    assert!(
        aside[0]["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("allowedClients: ")),
        "{row}"
    );
}

#[tokio::test]
async fn an_old_connector_above_a_refusing_ceiling_stays_aside_with_the_denial() {
    let (state, persons) = world(true).await;
    let who = format!("a{}", unique());
    let group = format!("g{}", unique());
    reads_mesto(&state, &who);
    client_policy(&state, &group, "refuse");
    keep(
        &state,
        &persons,
        &format!("sub-{who}"),
        &json!({ "connectors": [{ "name": "fav", "select": {} }] }),
    )
    .await;
    let moved = move_one(&state, &in_group(&who, &group))
        .await
        .expect("a move");
    assert_eq!((moved.moved, moved.empty), (0, 1), "{moved:?}");
    let row = stored(&state, &persons, &format!("sub-{who}")).await;
    let reason = row["connectorsNotMoved"][0]["reason"]
        .as_str()
        .expect("a reason");
    assert!(reason.contains("client"), "{row}");
}

#[tokio::test]
async fn what_stays_aside_is_added_to_what_an_earlier_move_kept_whatever_its_shape() {
    let (state, persons) = world(true).await;
    // `tajne` is a project this person reads nothing in: the connector takes nothing.
    let unreachable = json!({ "name": "secret-one", "select": { "projects": ["tajne"] } });
    for earlier in [json!("a note"), json!([{ "name": "older" }])] {
        let who = format!("a{}", unique());
        reads_mesto(&state, &who);
        keep(
            &state,
            &persons,
            &format!("sub-{who}"),
            &json!({ "connectors": [unreachable], "connectorsNotMoved": earlier }),
        )
        .await;
        let moved = move_one(&state, &common::person(&who))
            .await
            .expect("a move");
        assert_eq!(moved.empty, 1, "{moved:?}");
        let row = stored(&state, &persons, &format!("sub-{who}")).await;
        let kept = row["connectorsNotMoved"].as_array().expect("a list");
        assert_eq!(kept.len(), 2, "{row}");
        assert_eq!(kept[1]["name"], "secret-one");
        assert_eq!(kept[1]["reason"], "takes no Endpoint you may read today");
    }
}

#[tokio::test]
async fn an_empty_old_list_is_dropped_and_leaves_nothing_aside() {
    let (state, persons) = world(true).await;
    let who = format!("a{}", unique());
    keep(
        &state,
        &persons,
        &format!("sub-{who}"),
        &json!({ "theme": "dark", "connectors": [] }),
    )
    .await;
    let moved = move_one(&state, &common::person(&who))
        .await
        .expect("a move");
    assert_eq!(moved.moved, 0);
    assert_eq!(
        stored(&state, &persons, &format!("sub-{who}")).await,
        json!({ "theme": "dark" })
    );
}
