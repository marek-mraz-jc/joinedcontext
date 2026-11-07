//! `Group` through the Portal's own doors (T-0867, PF-62, PF-64, PF-52).
//!
//! The kind is organization-level, so it is read and written under the `org` namespace like
//! `Role` and `RoleBinding`: a caller who may read it sees the members, one who may not sees a
//! project that is not there, proposing needs `propose` on `Group`, and a binding to a group no
//! manifest declares is refused before a change exists.

mod common;

use axum::http::StatusCode;
use common::{envelope, person, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use serde_json::{json, Value};
use wiremock::matchers::{method as http_method, path as url_path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Group members are Keycloak user ids (ADR-N-061, PF-126).
const LEAD: &str = "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14";
const EVA: &str = "0b7e4d21-5c3a-4f9e-8d12-6a4b9c0e3f57";
const SECOND: &str = "7d9c2e80-1a4b-4c6d-9e3f-5b8a0c2d4e61";

/// A forge that takes the write and answers the open-pull list.
async fn forge() -> MockServer {
    let gitea = common::forge().await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&gitea)
        .await;
    gitea
}

/// A role at organization scope with these verbs on `Group`, bound to `who`.
fn bound(state: &joinedcontext_portal::state::AppState, who: &str, verbs: Value) {
    state.mirror.upsert(envelope(
        "Role",
        "group-keeper",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Group"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "keepers",
        ORG_NAMESPACE,
        json!({
            "role": "group-keeper",
            "subjects": [{ "user": format!("{who}@hel.fi") }],
            "scope": { "organization": "bb" }
        }),
    ));
}

fn group(name: &str, members: &[&str]) -> Value {
    json!({
        "apiVersion": joinedcontext_portal::resource::API_VERSION,
        "kind": "Group",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": {
            "description": "The people who lead the city's projects",
            "members": members.iter().map(|m| json!({ "user": m })).collect::<Vec<_>>()
        }
    })
}

#[tokio::test]
async fn a_reader_sees_the_members_and_a_stranger_sees_no_such_plural() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": LEAD }] }),
    ));
    bound(&state, "keeper", json!(["read"]));

    let answer = common::checked_send(
        &state,
        person("keeper"),
        "GET",
        "/api/v1/projects/org/groups",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(answer.text.contains(LEAD), "{}", answer.text);

    let one = common::checked_send(
        &state,
        person("keeper"),
        "GET",
        "/api/v1/projects/org/groups/city-leads",
        None,
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);

    // No binding at all: the kind is not there, not forbidden (PF-59, R20).
    let stranger = common::checked_send(
        &state,
        person("nobody"),
        "GET",
        "/api/v1/projects/org/groups",
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
    assert!(!stranger.text.contains(LEAD), "{}", stranger.text);
}

#[tokio::test]
async fn proposing_a_group_needs_propose_on_group_and_lands_in_the_red_lane() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    bound(&state, "keeper", json!(["read"]));

    // Read is not write, and the refusal says which verb is missing (PF-50).
    let refused = common::checked_send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/groups",
        Some(group("city-leads", &[LEAD])),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        refused.text.contains("propose") && refused.text.contains("Group"),
        "{}",
        refused.text
    );

    bound(&state, "keeper", json!(["read", "propose"]));
    let accepted = common::checked_send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/groups",
        Some(group("city-leads", &[LEAD])),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let change: Value = serde_json::from_str(&accepted.text).expect("a change");
    // Membership is reviewed like a Role and a RoleBinding (PF-52).
    assert_eq!(change["status"]["lane"], "red", "{}", accepted.text);
}

#[tokio::test]
async fn a_binding_to_a_group_no_manifest_declares_is_refused_before_a_change_exists() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Group", "RoleBinding", "Role"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admins",
        ORG_NAMESPACE,
        json!({
            "role": "admin",
            "subjects": [{ "user": "keeper@hel.fi" }],
            "scope": { "organization": "bb" }
        }),
    ));
    let binding = |group: &str| {
        json!({
            "apiVersion": joinedcontext_portal::resource::API_VERSION,
            "kind": "RoleBinding",
            "metadata": { "name": "leads-admin", "namespace": ORG_NAMESPACE },
            "spec": {
                "role": "admin",
                "subjects": [{ "group": group }],
                "scope": { "organization": "bb" }
            }
        })
    };

    let refused = common::checked_send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/rolebindings",
        Some(binding("city-leads")),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(
        refused.text.contains("city-leads") && refused.text.contains("no Group manifest"),
        "the refusal says what to do about it: {}",
        refused.text
    );

    // With the group declared, the same binding is taken.
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": LEAD }] }),
    ));
    let accepted = common::checked_send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/rolebindings",
        Some(binding("city-leads")),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
}

/// AP-115: a Group the realm holds unmanaged, or an App's default group written over the
/// organization's own, is refused at the propose door naming both owners; the same group
/// written again under its owner passes.
#[tokio::test]
async fn a_group_name_another_owner_holds_is_refused_naming_both() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    bound(&state, "keeper", json!(["propose", "read"]));
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": LEAD }] }),
    ));
    state
        .foreign_names
        .set_groups(std::collections::BTreeSet::from(["admins".to_owned()]));
    let groups = "/api/v1/projects/org/groups?dryRun=All";

    let foreign = common::checked_send(
        &state,
        person("keeper"),
        "POST",
        groups,
        Some(group("admins", &[LEAD])),
    )
    .await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN, "{}", foreign.text);
    assert!(
        foreign.text.contains("'admins' for the organization")
            && foreign.text.contains("never taken over"),
        "{}",
        foreign.text
    );

    let mut default_group = group("city-leads", &[LEAD]);
    default_group["metadata"]["annotations"] = json!({ "joinedcontext.com/app": "doprava/city" });
    let clash = common::checked_send(
        &state,
        person("keeper"),
        "PUT",
        "/api/v1/projects/org/groups/city-leads?dryRun=All",
        Some(default_group),
    )
    .await;
    assert_eq!(clash.status, StatusCode::FORBIDDEN, "{}", clash.text);
    assert!(
        clash.text.contains("belongs to the organization")
            && clash.text.contains("the App city of project doprava")
            && clash.text.contains("AP-115"),
        "{}",
        clash.text
    );

    let same = common::checked_send(
        &state,
        person("keeper"),
        "PUT",
        "/api/v1/projects/org/groups/city-leads?dryRun=All",
        Some(group("city-leads", &[LEAD, SECOND])),
    )
    .await;
    assert_eq!(same.status, StatusCode::OK, "{}", same.text);
}

/// AP-119: a project's steward reads and proposes the members of their App's default group from
/// the App page, and no other group of the organization.
#[tokio::test]
async fn an_apps_steward_keeps_its_default_group_and_no_other() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "App",
        "board",
        "doprava",
        json!({ "class": "ui" }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "app-steward",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "app-stewards",
        ORG_NAMESPACE,
        json!({
            "role": "app-steward",
            "subjects": [{ "user": "steward@hel.fi" }],
            "scope": { "project": "doprava" }
        }),
    ));
    let mut board = envelope(
        "Group",
        "board-viewer",
        ORG_NAMESPACE,
        json!({ "description": "The people who lead the city's projects", "members": [] }),
    );
    board
        .metadata
        .annotations
        .insert("joinedcontext.com/app".into(), "doprava/board".into());
    state.mirror.upsert(board);
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": LEAD }] }),
    ));

    let read = common::checked_send(
        &state,
        person("steward"),
        "GET",
        "/api/v1/projects/org/groups/board-viewer",
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let other = common::checked_send(
        &state,
        person("steward"),
        "GET",
        "/api/v1/projects/org/groups/city-leads",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);

    let mut added = group("board-viewer", &[EVA]);
    added["metadata"]["annotations"] = json!({ "joinedcontext.com/app": "doprava/board" });
    let proposed = common::checked_send(
        &state,
        person("steward"),
        "PUT",
        "/api/v1/projects/org/groups/board-viewer",
        Some(added),
    )
    .await;
    assert_eq!(proposed.status, StatusCode::ACCEPTED, "{}", proposed.text);

    // Anything beside the members stays the organization's (F-POR4-010).
    let mut described = group("board-viewer", &[EVA]);
    described["metadata"]["annotations"] = json!({ "joinedcontext.com/app": "doprava/board" });
    described["spec"]["description"] = json!("now also the operators' group");
    let redescribed = common::checked_send(
        &state,
        person("steward"),
        "PUT",
        "/api/v1/projects/org/groups/board-viewer",
        Some(described),
    )
    .await;
    assert_eq!(
        redescribed.status,
        StatusCode::FORBIDDEN,
        "{}",
        redescribed.text
    );

    let refused = common::checked_send(
        &state,
        person("steward"),
        "PUT",
        "/api/v1/projects/org/groups/city-leads",
        Some(group("city-leads", &[LEAD, EVA])),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
}
