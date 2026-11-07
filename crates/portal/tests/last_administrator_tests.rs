//! PF-03: an organization keeps at least one administrator. The change that would remove the
//! last one — deleting its binding, pointing it at a lesser role, emptying its subjects, or
//! taking `delete` on `RoleBinding` out of its role — is refused with a reason a person can act
//! on, and every change that leaves an administrator standing passes.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::MockServer;

use common::{envelope, person};
use joinedcontext_portal::api::delete::delete_with_identity;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::permissions::{keeps_an_administrator, AccessChange, ORG_NAMESPACE};
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

fn admin_rules() -> Value {
    json!([{ "kinds": ["Role", "RoleBinding", "Endpoint"], "verbs": ["propose", "approve", "delete"] }])
}

/// The organization's roles, and one binding per `(name, role)` at organization scope.
fn organization(bindings: &[(&str, &str)]) -> Mirror {
    let mirror = Mirror::new();
    mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": admin_rules() }),
    ));
    mirror.upsert(envelope(
        "Role",
        "viewer",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    ));
    for (name, role) in bindings {
        mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{name}@hel.fi") }], "role": role,
                    "scope": { "organization": "hel" } }),
        ));
    }
    mirror
}

fn binding(name: &str, role: &str, subjects: Value) -> Value {
    json!({
        "apiVersion": API_VERSION, "kind": "RoleBinding",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": { "subjects": subjects, "role": role, "scope": { "organization": "hel" } },
    })
}

fn refused(result: Result<(), ApiError>) -> String {
    match result {
        Err(ApiError::Conflict(detail)) => detail,
        other => panic!("the last administrator was let go: {other:?}"),
    }
}

fn remove<'a>(kind: &'a str, name: &'a str) -> AccessChange<'a> {
    AccessChange::Remove {
        kind,
        namespace: ORG_NAMESPACE,
        name,
    }
}

#[test]
fn the_last_administrators_binding_is_not_deleted() {
    let mirror = organization(&[("admin", "org-admin"), ("reader", "viewer")]);
    let detail = refused(keeps_an_administrator(
        &mirror,
        remove("RoleBinding", "admin"),
    ));
    assert!(
        detail.contains("PF-03") && detail.contains("admin"),
        "{detail}"
    );
    assert!(
        detail.contains("Bind another person"),
        "the reason says what to do: {detail}"
    );
    // A binding that makes nobody an administrator goes as it always did.
    assert!(keeps_an_administrator(&mirror, remove("RoleBinding", "reader")).is_ok());
}

#[test]
fn one_of_two_administrators_may_go() {
    let mirror = organization(&[("admin", "org-admin"), ("deputy", "org-admin")]);
    assert!(keeps_an_administrator(&mirror, remove("RoleBinding", "admin")).is_ok());
}

#[test]
fn the_last_administrator_is_not_demoted_or_emptied_by_an_edit() {
    let mirror = organization(&[("admin", "org-admin")]);
    let demoted = binding("admin", "viewer", json!([{ "user": "admin@hel.fi" }]));
    refused(keeps_an_administrator(
        &mirror,
        AccessChange::Write(&demoted),
    ));
    let emptied = binding("admin", "org-admin", json!([]));
    refused(keeps_an_administrator(
        &mirror,
        AccessChange::Write(&emptied),
    ));
    // Adding a second administrator is fine, and so is changing who the one binding names.
    let second = binding("deputy", "org-admin", json!([{ "user": "deputy@hel.fi" }]));
    assert!(keeps_an_administrator(&mirror, AccessChange::Write(&second)).is_ok());
    let handed_over = binding(
        "admin",
        "org-admin",
        json!([{ "user": "successor@hel.fi" }]),
    );
    assert!(keeps_an_administrator(&mirror, AccessChange::Write(&handed_over)).is_ok());
}

#[test]
fn the_administrators_role_does_not_lose_what_makes_it_one() {
    let mirror = organization(&[("admin", "org-admin")]);
    let weakened = json!({
        "apiVersion": API_VERSION, "kind": "Role",
        "metadata": { "name": "org-admin", "namespace": ORG_NAMESPACE },
        "spec": { "rules": [{ "kinds": ["Role", "RoleBinding"], "verbs": ["propose", "approve"] }] },
    });
    refused(keeps_an_administrator(
        &mirror,
        AccessChange::Write(&weakened),
    ));
    refused(keeps_an_administrator(&mirror, remove("Role", "org-admin")));
}

#[test]
fn an_organization_on_its_bootstrap_group_is_not_held_to_a_first_administrator() {
    // No binding administers yet: the guard keeps the last one, it does not demand a first.
    let mirror = organization(&[("reader", "viewer")]);
    assert!(keeps_an_administrator(&mirror, remove("RoleBinding", "reader")).is_ok());
    // A project's own bindings are not the organization's administrators.
    let project = binding("steward", "viewer", json!([{ "user": "x@hel.fi" }]));
    let mut in_project = project.clone();
    in_project["metadata"]["namespace"] = json!("helsinki");
    assert!(keeps_an_administrator(
        &organization(&[("admin", "org-admin")]),
        AccessChange::Write(&in_project)
    )
    .is_ok());
}

/// The delete route refuses it too, before any change is opened, with 409.
#[tokio::test]
async fn the_delete_door_refuses_the_last_administrator_with_409() {
    let forge = MockServer::start().await;
    let state: AppState = common::state_on(&forge);
    state
        .mirror
        .replace_all(&organization(&[("admin", "org-admin")]));
    let admin = person("admin");
    let refused = delete_with_identity(
        &admin,
        &state,
        ORG_NAMESPACE,
        "rolebindings",
        "admin",
        true,
        None,
    )
    .await
    .expect_err("the last administrator's binding cannot be deleted");
    assert!(
        matches!(&refused, ApiError::Conflict(detail) if detail.contains("PF-03")),
        "{refused:?}"
    );
    use axum::response::IntoResponse;
    assert_eq!(refused.into_response().status(), StatusCode::CONFLICT);
}

/// With several Organizations (ADR-N-058) only the installation's administrators keep it
/// running: another Organization's admin binding does not let its last one go (T-5241, T-5248).
#[test]
fn another_organizations_administrator_does_not_replace_the_installations_last() {
    let mirror = organization(&[("admin", "org-admin")]);
    for (name, installation) in [("hel", true), ("zilina", false)] {
        mirror.upsert(envelope(
            "Organization",
            name,
            ORG_NAMESPACE,
            json!({ "domain": format!("{name}.sk"), "installation": installation }),
        ));
    }
    mirror.upsert(envelope(
        "RoleBinding",
        "zilina-admins",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "eva@zilina.sk" }], "role": "org-admin",
                "scope": { "organization": "zilina" } }),
    ));
    let detail = refused(keeps_an_administrator(
        &mirror,
        remove("RoleBinding", "admin"),
    ));
    assert!(detail.contains("admin"), "{detail}");
    assert!(keeps_an_administrator(&mirror, remove("RoleBinding", "zilina-admins")).is_ok());
}
