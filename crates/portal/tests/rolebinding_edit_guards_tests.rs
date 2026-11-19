//! T-7403: an Organization's administrator who edits a `RoleBinding` as free text (the Members
//! page's YAML) is held by the server: no other Organization's scope or project, no role above
//! their own rights, no edit or removal that leaves their Organization without an administrator,
//! a second factor before they change their own binding (IDN-06), and a member who administers
//! nothing hears the same 403 whether the binding exists or not.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use jc_core::kinds::Verb;
use serde_json::{json, Value};
use wiremock::MockServer;

use common::envelope;
use joinedcontext_portal::api::delete::delete_with_identity;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::permissions::{for_request, within_own_rights, ORG_NAMESPACE};
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const BBSK_PROJECT: &str = "bbsk-air";
const PRAHA_PROJECT: &str = "praha-bus";

fn someone(name: &str, second_factor: bool) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor,
    }
}

/// The only administrator of `bbsk`, signed in with a password alone.
fn bbsk_admin() -> Identity {
    someone("bbadmin", false)
}

fn binding(name: &str, user: &str, role: &str, scope: Value, namespace: &str) -> Value {
    json!({
        "apiVersion": API_VERSION, "kind": "RoleBinding",
        "metadata": { "name": name, "namespace": namespace },
        "spec": { "subjects": [{ "user": format!("{user}@example.org") }], "role": role, "scope": scope },
    })
}

/// The admin's own binding as the edit page loads it.
fn own_binding() -> Value {
    binding(
        "bbsk-admins",
        "bbadmin",
        "org-admin",
        json!({ "organization": "bbsk" }),
        ORG_NAMESPACE,
    )
}

/// Three Organizations: the installation's `ops`, `bbsk` with one administrator and a steward,
/// and `praha` with its own administrator. `installation-admin` grants what only operators hold.
async fn world(second_factor: bool) -> (AppState, MockServer) {
    let forge = MockServer::start().await;
    let mut state: AppState = common::state_on(&forge);
    let mut config = Config::for_tests();
    config.decisions_need_second_factor = second_factor;
    state.config = Arc::new(config);
    let m = &state.mirror;
    for (name, spec) in [
        (
            "ops",
            json!({ "domain": "ops.example", "installation": true }),
        ),
        ("bbsk", json!({ "domain": "bbsk.sk" })),
        ("praha", json!({ "domain": "praha.eu" })),
    ] {
        m.upsert(envelope("Organization", name, ORG_NAMESPACE, spec));
    }
    for (project, org) in [(BBSK_PROJECT, "bbsk"), (PRAHA_PROJECT, "praha")] {
        m.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({ "organizationRef": org }),
        ));
    }
    let admin = json!({ "rules": [{ "kinds": ["Role", "RoleBinding", "Group", "Organization", "Endpoint"],
                                    "verbs": ["read", "propose", "approve", "delete"] }] });
    let every: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    for (role, spec) in [
        ("org-admin", admin),
        (
            "installation-admin",
            json!({ "rules": [{ "kinds": every, "verbs": ["read", "propose", "approve", "delete"] }] }),
        ),
        (
            "viewer",
            json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
        ),
    ] {
        m.upsert(envelope("Role", role, ORG_NAMESPACE, spec));
    }
    for b in [
        binding(
            "operators",
            "operator",
            "installation-admin",
            json!({ "organization": "ops" }),
            ORG_NAMESPACE,
        ),
        own_binding(),
        binding(
            "bbsk-air-viewer",
            "bbviewer",
            "viewer",
            json!({ "project": BBSK_PROJECT }),
            ORG_NAMESPACE,
        ),
        binding(
            "praha-admins",
            "prahaadmin",
            "org-admin",
            json!({ "organization": "praha" }),
            ORG_NAMESPACE,
        ),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            b["metadata"]["name"].as_str().unwrap_or_default(),
            ORG_NAMESPACE,
            b["spec"].clone(),
        ));
    }
    (state, forge)
}

/// Both gates every RoleBinding write passes (UI save, API, import, MCP, Change apply): the
/// proposer's rule check, then `within_own_rights`.
fn save(state: &AppState, who: &Identity, manifest: &Value) -> Result<(), ApiError> {
    let namespace = manifest["metadata"]["namespace"]
        .as_str()
        .unwrap_or(ORG_NAMESPACE);
    for_request(state, who, namespace).check("RoleBinding", Verb::Propose, Some(manifest))?;
    within_own_rights(state, who, manifest, None, "proposer")
}

fn status(error: ApiError) -> StatusCode {
    error.into_response().status()
}

/// Item 1: another Organization's scope, a project of another Organization, a binding written
/// into another Organization's project and a role above the admin's own rights are each refused.
#[tokio::test]
async fn an_administrator_may_not_point_a_binding_outside_their_organization_or_rights() {
    let (state, _forge) = world(false).await;
    let admin = bbsk_admin();
    let mut other_org = own_binding();
    other_org["spec"]["scope"] = json!({ "organization": "praha" });
    let mut other_project = own_binding();
    other_project["spec"]["scope"] = json!({ "project": PRAHA_PROJECT });
    let mut other_namespace = own_binding();
    other_namespace["metadata"]["namespace"] = json!(PRAHA_PROJECT);
    other_namespace["spec"]["scope"] = json!({ "project": PRAHA_PROJECT });
    let mut operator_role = own_binding();
    operator_role["spec"]["role"] = json!("installation-admin");
    for (case, manifest) in [
        ("scope.organization", other_org),
        ("scope.project", other_project),
        ("metadata.namespace", other_namespace),
        ("role", operator_role),
    ] {
        let refusal = save(&state, &admin, &manifest).expect_err(case);
        assert_eq!(status(refusal), StatusCode::FORBIDDEN, "{case}");
    }
    // A role the admin holds, at their own Organization, still passes.
    let mut viewer = own_binding();
    viewer["metadata"]["name"] = json!("bbsk-viewer");
    viewer["spec"]["role"] = json!("viewer");
    assert!(save(&state, &admin, &viewer).is_ok());
}

/// Item 2: `bbsk` has one administrator, and `bbsk` is not the installation: demoting, emptying or
/// deleting that binding is refused 409 naming the Organization, as the installation's is.
#[tokio::test]
async fn the_last_administrator_of_an_organization_is_not_demoted_or_removed() {
    let (state, _forge) = world(false).await;
    let admin = bbsk_admin();
    let mut demoted = own_binding();
    demoted["spec"]["role"] = json!("viewer");
    let mut emptied = own_binding();
    emptied["spec"]["subjects"] = json!([]);
    for (case, manifest) in [("role", demoted), ("subjects", emptied)] {
        let refusal =
            within_own_rights(&state, &admin, &manifest, None, "proposer").expect_err(case);
        assert!(
            matches!(&refusal, ApiError::Conflict(d) if d.contains("Organization bbsk") && d.contains("PF-03")),
            "{case}: {refusal:?}"
        );
    }
    let refusal = delete_with_identity(
        &admin,
        &state,
        ORG_NAMESPACE,
        "rolebindings",
        "bbsk-admins",
        true,
        None,
    )
    .await
    .expect_err("the last administrator's binding stays");
    assert!(
        matches!(&refusal, ApiError::Conflict(d) if d.contains("Organization bbsk")),
        "{refusal:?}"
    );
    assert_eq!(status(refusal), StatusCode::CONFLICT);
}

/// Item 3: with a second administrator in `bbsk`, a change to one's own binding (edit or delete)
/// asks for the second factor; the same session edits someone else's binding, and a session that
/// proved the factor edits its own.
#[tokio::test]
async fn a_change_to_ones_own_binding_asks_for_the_second_factor() {
    let (state, _forge) = world(true).await;
    let second = binding(
        "bbsk-admins-2",
        "eva",
        "org-admin",
        json!({ "organization": "bbsk" }),
        ORG_NAMESPACE,
    );
    state.mirror.upsert(envelope(
        "RoleBinding",
        "bbsk-admins-2",
        ORG_NAMESPACE,
        second["spec"].clone(),
    ));
    let mut widened = own_binding();
    widened["spec"]["subjects"] =
        json!([{ "user": "bbadmin@example.org" }, { "user": "jan@example.org" }]);
    let refusal = save(&state, &bbsk_admin(), &widened).expect_err("own binding");
    assert!(
        matches!(refusal, ApiError::SecondFactorRequired),
        "{refusal:?}"
    );
    // Removing oneself from the binding is a change of one's own binding too.
    let mut left = own_binding();
    left["spec"]["subjects"] = json!([{ "user": "jan@example.org" }]);
    let refusal = save(&state, &bbsk_admin(), &left).expect_err("leaving it");
    assert!(
        matches!(refusal, ApiError::SecondFactorRequired),
        "{refusal:?}"
    );
    let refusal = delete_with_identity(
        &bbsk_admin(),
        &state,
        ORG_NAMESPACE,
        "rolebindings",
        "bbsk-admins",
        true,
        None,
    )
    .await
    .expect_err("deleting it");
    assert!(
        matches!(refusal, ApiError::SecondFactorRequired),
        "{refusal:?}"
    );

    let mut others = second.clone();
    others["spec"]["subjects"] =
        json!([{ "user": "eva@example.org" }, { "user": "jan@example.org" }]);
    assert!(save(&state, &bbsk_admin(), &others).is_ok());
    assert!(save(&state, &someone("bbadmin", true), &widened).is_ok());
}

/// Item 4: a member who administers nothing is refused 403 on an existing binding and on a name
/// that exists nowhere, with the same answer, so the refusal tells nothing about the bindings.
#[tokio::test]
async fn a_member_who_administers_nothing_is_refused_alike_on_any_binding() {
    let (state, _forge) = world(false).await;
    let viewer = someone("bbviewer", false);
    let mut missing = own_binding();
    missing["metadata"]["name"] = json!("no-such-binding");
    let existing = save(&state, &viewer, &own_binding()).expect_err("existing");
    let absent = save(&state, &viewer, &missing).expect_err("absent");
    assert_eq!(existing.to_string(), absent.to_string());
    assert_eq!(status(existing), StatusCode::FORBIDDEN);
    assert_eq!(status(absent), StatusCode::FORBIDDEN);
}
