//! Nobody grants above their own rights (T-0740, PF-52, AG-77, CC-19): a `Role` or `RoleBinding`
//! proposed, dry-run or approved through the REST routes or the operations registry is checked
//! against the person on the scope it applies to. Every cell of the refusal table is a case here.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{checked_send as send, encode, envelope, person, Answer, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const PROPOSE: &str = "/api/v1/projects/org/rolebindings";
const ROLES: &str = "/api/v1/projects/org/roles";
const OPS: &str = "/api/v1/projects/org/ops";

/// A binding someone else proposed of `org-admin` over the organization, and the removal of the
/// steward binding on helsinki, and a binding of `steward` on helsinki.
const ADMIN_GRANT: u64 = 0x301;
const REMOVAL: u64 = 0x302;
const STEWARD_GRANT: u64 = 0x303;
/// Jana's Keycloak user id: a binding names a person by id (ADR-N-061, PF-126).
const JANA_ID: &str = "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14";

fn binding(name: &str, role: &str, scope: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": { "subjects": [{ "user": JANA_ID }], "role": role, "scope": scope },
    })
}

fn role(name: &str, rules: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Role",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": { "rules": rules },
    })
}

fn organization() -> Value {
    json!({ "organization": "hel" })
}

fn project(name: &str) -> Value {
    json!({ "project": name })
}

/// The organization's roles and who holds them:
/// - `admin`: org-admin over the organization, the one who holds everything.
/// - `steward`: steward over the organization, service accounts included, no verb on bindings.
/// - `lead`: binder over the organization (proposes and approves roles and bindings) and steward
///   on the helsinki project only.
/// - `approver`: approves every kind over the organization and proposes nothing.
/// - `limited`: proposes bindings, and Endpoints that are not public.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    let kinds = json!(["Endpoint", "Pipeline", "Role", "RoleBinding"]);
    for (name, rules) in [
        (
            "org-admin",
            json!([{ "kinds": kinds, "verbs": ["propose", "approve", "delete"] }]),
        ),
        (
            "steward",
            json!([{ "kinds": ["Endpoint", "Pipeline", "ServiceAccount"], "verbs": ["propose", "approve"] }]),
        ),
        (
            "binder",
            json!([{ "kinds": ["Role", "RoleBinding"], "verbs": ["propose", "approve"] }]),
        ),
        (
            "approver",
            json!([{ "kinds": kinds, "verbs": ["approve"] }]),
        ),
        (
            "internal-editor",
            json!([
                { "kinds": ["RoleBinding"], "verbs": ["propose"] },
                { "kinds": ["Endpoint"], "verbs": ["propose"],
                  "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
            ]),
        ),
        (
            "endpoint-editor",
            json!([{ "kinds": ["Endpoint"], "verbs": ["propose"] }]),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            name,
            ORG_NAMESPACE,
            json!({ "rules": rules }),
        ));
    }
    for (holder, role, scope) in [
        ("admin", "org-admin", organization()),
        ("steward", "steward", organization()),
        ("lead", "binder", organization()),
        ("lead", "steward", project("helsinki")),
        ("approver", "approver", organization()),
        ("limited", "internal-editor", organization()),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{holder}-{role}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{holder}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    for (space, project) in [("helsinki", "helsinki"), ("espoo", "espoo")] {
        state.mirror.upsert(envelope(
            "ContextSpace",
            space,
            project,
            json!({ "isSandbox": false }),
        ));
    }
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-steward",
        ORG_NAMESPACE,
        binding("jana-steward", "steward", project("helsinki"))["spec"].clone(),
    ));
    state
}

/// A merge request by `someone@hel.fi` on `branch`, whose head (or, for a removal, base) holds
/// `manifest` at `file`.
async fn pull(gitea: &MockServer, number: u64, branch: &str, file: &str, manifest: &Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/{number}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": number,
            "html_url": format!("https://gitea.example/pulls/{number}"),
            "state": "open",
            "title": branch,
            "head": { "ref": branch },
            "base": { "ref": "main" },
            "created_at": "2026-09-15T09:00:00Z",
            "user": { "login": "someone", "full_name": "someone", "email": "someone@hel.fi" },
            "mergeable": true,
            "merged": false
        })))
        .mount(gitea)
        .await;
    let git_ref = if branch.starts_with("portal/delete-") {
        "main"
    } else {
        branch
    };
    let status = if git_ref == "main" {
        "deleted"
    } else {
        "added"
    };
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/{number}/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "filename": file, "status": status }
        ])))
        .mount(gitea)
        .await;
    let yaml = serde_norway::to_string(manifest).expect("yaml");
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{file}")))
        .and(query_param("ref", git_ref))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": format!("sha-{number}"), "content": encode(&yaml)
        })))
        .with_priority(1)
        .mount(gitea)
        .await;
}

async fn forge() -> MockServer {
    let gitea = common::forge().await;
    pull(
        &gitea,
        ADMIN_GRANT,
        "portal/create-rolebinding-jana-admin-00000301",
        "users/assignments/jana-admin.yaml",
        &binding("jana-admin", "org-admin", organization()),
    )
    .await;
    pull(
        &gitea,
        REMOVAL,
        "portal/delete-rolebinding-jana-steward-00000302",
        "users/assignments/jana-steward.yaml",
        &binding("jana-steward", "steward", project("helsinki")),
    )
    .await;
    pull(
        &gitea,
        STEWARD_GRANT,
        "portal/create-rolebinding-jana-helsinki-00000303",
        "users/assignments/jana-helsinki.yaml",
        &binding("jana-helsinki", "steward", project("helsinki")),
    )
    .await;
    gitea
}

fn refused(answer: &Answer, words: &str) {
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains(words),
        "expected «{words}» in {}",
        answer.text
    );
}

fn red_change(answer: &Answer) {
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change: Value = serde_json::from_str(&answer.text).expect("change");
    assert_eq!(change["status"]["lane"], "red", "{}", answer.text);
}

#[tokio::test]
async fn a_binding_is_refused_when_its_role_names_a_verb_its_proposer_lacks_on_its_scope() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let lead = || person("lead");

    // A steward holds no verb on bindings at all.
    let answer = send(
        &state,
        person("steward"),
        "POST",
        PROPOSE,
        Some(binding("jana-admin", "org-admin", project("helsinki"))),
    )
    .await;
    refused(&answer, "no role grants propose on RoleBinding");

    // The lead is steward on helsinki: org-admin there names delete, Role and RoleBinding verbs
    // the lead lacks, each named once.
    let answer = send(
        &state,
        lead(),
        "POST",
        PROPOSE,
        Some(binding("jana-admin", "org-admin", project("helsinki"))),
    )
    .await;
    refused(
        &answer,
        "a binding may not grant more than its proposer holds: missing delete on Endpoint",
    );
    assert!(
        answer.text.contains("delete on RoleBinding"),
        "{}",
        answer.text
    );
    assert_eq!(answer.text.matches("delete on Endpoint").count(), 1);

    // Steward on helsinki is within the lead's rights, on the project and on one of its spaces.
    let answer = send(
        &state,
        lead(),
        "POST",
        PROPOSE,
        Some(binding("jana-helsinki", "steward", project("helsinki"))),
    )
    .await;
    red_change(&answer);
    let answer = send(
        &state,
        lead(),
        "POST",
        PROPOSE,
        Some(binding(
            "jana-space",
            "steward",
            json!({ "contextSpace": "helsinki" }),
        )),
    )
    .await;
    red_change(&answer);

    // The same role over the organization, another project or another project's space is not.
    for (name, scope) in [
        ("jana-org", organization()),
        ("jana-espoo", project("espoo")),
        ("jana-espoo-space", json!({ "contextSpace": "espoo" })),
    ] {
        let answer = send(
            &state,
            lead(),
            "POST",
            PROPOSE,
            Some(binding(name, "steward", scope)),
        )
        .await;
        refused(
            &answer,
            "missing propose on Endpoint, approve on Endpoint, propose on Pipeline, approve on Pipeline, propose on ServiceAccount",
        );
    }

    // A dry run answers the same, so the check a form or the assistant runs first refuses too.
    let answer = send(
        &state,
        lead(),
        "POST",
        &format!("{PROPOSE}?dryRun=All"),
        Some(binding("jana-org", "steward", organization())),
    )
    .await;
    refused(&answer, "may not grant more than its proposer holds");

    // A binding to a role the organization lacks would grant whatever that role becomes.
    let answer = send(
        &state,
        lead(),
        "POST",
        PROPOSE,
        Some(binding("jana-ghost", "ghost", project("helsinki"))),
    )
    .await;
    refused(
        &answer,
        "no role ghost is defined where this binding applies",
    );

    // The administrator holds everything over the organization: the grant is a red change.
    let answer = send(
        &state,
        person("admin"),
        "POST",
        PROPOSE,
        Some(binding("jana-admin", "org-admin", organization())),
    )
    .await;
    red_change(&answer);
}

#[tokio::test]
async fn a_constraint_the_proposer_is_held_to_cannot_be_dropped_by_the_role_they_grant() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let answer = send(
        &state,
        person("limited"),
        "POST",
        PROPOSE,
        Some(binding(
            "jana-internal",
            "internal-editor",
            project("helsinki"),
        )),
    )
    .await;
    red_change(&answer);

    let answer = send(
        &state,
        person("limited"),
        "POST",
        PROPOSE,
        Some(binding(
            "jana-endpoints",
            "endpoint-editor",
            project("helsinki"),
        )),
    )
    .await;
    refused(&answer, "missing propose on Endpoint (PF-52)");
}

#[tokio::test]
async fn a_role_is_checked_against_its_proposer_over_the_whole_organization() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let publisher = role(
        "endpoint-publisher",
        json!([{ "kinds": ["Endpoint"], "verbs": ["propose", "approve"] }]),
    );

    // The lead proposes roles, but holds Endpoint verbs on helsinki only.
    let answer = send(
        &state,
        person("lead"),
        "POST",
        ROLES,
        Some(publisher.clone()),
    )
    .await;
    refused(
        &answer,
        "a role may not grant more than its proposer holds: missing propose on Endpoint, approve on Endpoint",
    );
    let answer = send(
        &state,
        person("steward"),
        "POST",
        ROLES,
        Some(publisher.clone()),
    )
    .await;
    refused(&answer, "no role grants propose on Role");
    let answer = send(&state, person("admin"), "POST", ROLES, Some(publisher)).await;
    red_change(&answer);
}

#[tokio::test]
async fn a_service_account_holds_no_role_of_the_organization_its_proposer_lacks() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let account = |role: &str| {
        json!({
            "apiVersion": API_VERSION,
            "kind": "ServiceAccount",
            "metadata": { "name": "bike-sync", "namespace": "helsinki" },
            "spec": {
                "owner": { "user": "steward" },
                "purpose": "syncs the bike stations",
                "roles": [{ "role": role, "scope": { "project": "helsinki" } }],
                "credentials": [{ "kind": "oauth-client", "name": "main" }]
            }
        })
    };
    let accounts = "/api/v1/projects/helsinki/serviceaccounts";

    let answer = send(
        &state,
        person("steward"),
        "POST",
        accounts,
        Some(account("org-admin")),
    )
    .await;
    refused(
        &answer,
        "a service account may not grant more than its proposer holds: missing delete on Endpoint",
    );
    // A role of its own and a role template of the gateway (granting data access through
    // Policies, not Portal verbs) are within the steward's rights.
    for role in ["steward", "space-writer"] {
        let answer = send(
            &state,
            person("steward"),
            "POST",
            accounts,
            Some(account(role)),
        )
        .await;
        red_change(&answer);
    }
}

#[tokio::test]
async fn the_operations_registry_refuses_the_same_grant_in_the_same_words() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let answer = send(
        &state,
        person("lead"),
        "POST",
        &format!("{OPS}/jc_resource_propose"),
        Some(json!({ "manifest": binding("jana-admin", "org-admin", project("helsinki")) })),
    )
    .await;
    refused(
        &answer,
        "a binding may not grant more than its proposer holds: missing delete on Endpoint",
    );

    let answer = send(
        &state,
        person("approver"),
        "POST",
        &format!("{OPS}/jc_change_approve"),
        Some(json!({ "id": format!("chg-{ADMIN_GRANT:08x}"), "confirm": "jana-admin" })),
    )
    .await;
    refused(
        &answer,
        "a binding may not grant more than its approver holds",
    );
}

#[tokio::test]
async fn an_approver_approves_a_grant_only_within_their_own_rights_and_a_removal_only_with_delete()
{
    let gitea = forge().await;
    let state = state_with(&gitea);
    let approve = |number: u64| format!("/api/v1/projects/org/changes/chg-{number:08x}/approve");

    // Approving org-admin over the organization needs every verb of it.
    for who in ["approver", "lead"] {
        let answer = send(
            &state,
            person(who),
            "POST",
            &approve(ADMIN_GRANT),
            Some(json!({ "confirm": "jana-admin" })),
        )
        .await;
        refused(
            &answer,
            "a binding may not grant more than its approver holds: missing",
        );
    }

    // Removing a binding needs delete on RoleBinding as well as approve.
    for who in ["approver", "lead"] {
        let answer = send(
            &state,
            person(who),
            "POST",
            &approve(REMOVAL),
            Some(json!({ "confirm": "jana-steward" })),
        )
        .await;
        refused(&answer, "no role grants delete on RoleBinding");
    }

    // An access change is red: the name is typed back before anything merges.
    let answer = send(
        &state,
        person("admin"),
        "POST",
        &approve(ADMIN_GRANT),
        Some(json!({})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(
        answer.text.contains("confirm to be 'jana-admin'"),
        "{}",
        answer.text
    );

    for (who, number, name) in [
        ("admin", ADMIN_GRANT, "jana-admin"),
        ("admin", REMOVAL, "jana-steward"),
        ("lead", STEWARD_GRANT, "jana-helsinki"),
    ] {
        let answer = send(
            &state,
            person(who),
            "POST",
            &approve(number),
            Some(json!({ "confirm": name })),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::ACCEPTED,
            "{who} approving {name}: {}",
            answer.text
        );
    }
    assert!(gitea
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .any(|request| request.url.path().ends_with("/merge")));
}

#[tokio::test]
async fn the_bootstrap_group_grants_the_first_administrator_into_an_empty_repository() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Role", "RoleBinding"], "verbs": ["propose", "approve", "delete"] }] }),
    ));
    let mut bootstrap = person("first");
    bootstrap.groups = vec![state.config.bootstrap_admins.clone()];

    let answer = send(
        &state,
        bootstrap,
        "POST",
        PROPOSE,
        Some(binding("first-admin", "org-admin", organization())),
    )
    .await;
    red_change(&answer);
}

// ---------------------------------------------------------------------------
// A role of a project, written and bound inside it alone (PF-68, PF-69, PF-70, T-0872)
// ---------------------------------------------------------------------------

/// A `Role` manifest in a project's namespace, proposed at that project's own roles path.
fn project_role(project: &str, name: &str, rules: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Role",
        "metadata": { "name": name, "namespace": project },
        "spec": { "rules": rules },
    })
}

#[tokio::test]
async fn a_project_role_may_not_grant_a_verb_its_proposer_lacks_in_that_project() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    // The lead is steward on helsinki: propose and approve on Endpoint and Pipeline there, and
    // no delete anywhere. A role of helsinki granting delete is above their own rights.
    let answer = send(
        &state,
        person("lead"),
        "POST",
        "/api/v1/projects/helsinki/roles",
        Some(project_role(
            "helsinki",
            "cleaner",
            json!([{ "kinds": ["Pipeline"], "verbs": ["propose", "delete"] }]),
        )),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains("delete on Pipeline"),
        "the refusal names the verb: {}",
        answer.text
    );

    // The same role without the verb they lack is theirs to write.
    let answer = send(
        &state,
        person("lead"),
        "POST",
        "/api/v1/projects/helsinki/roles",
        Some(project_role(
            "helsinki",
            "pipeline-writer",
            json!([{ "kinds": ["Pipeline"], "verbs": ["propose"] }]),
        )),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);

    // And the same role in a project where they are nothing is refused there.
    let answer = send(
        &state,
        person("lead"),
        "POST",
        "/api/v1/projects/espoo/roles",
        Some(project_role(
            "espoo",
            "pipeline-writer",
            json!([{ "kinds": ["Pipeline"], "verbs": ["propose"] }]),
        )),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
}

#[tokio::test]
async fn a_role_of_one_project_grants_nothing_in_another_and_nothing_at_organization_scope() {
    use joinedcontext_portal::permissions;

    let gitea = forge().await;
    let state = state_with(&gitea);
    // helsinki's own role, and jana bound to it in helsinki.
    state.mirror.upsert(envelope(
        "Role",
        "air-analyst",
        "helsinki",
        json!({ "rules": [{ "kinds": ["DataSource"], "verbs": ["propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-analyst",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana.kovacova@hel.fi" }],
            "role": "air-analyst",
            "scope": { "project": "helsinki" }
        }),
    ));
    // The same name bound in espoo, where no such role exists, and over the organization.
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-analyst-espoo",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana.kovacova@hel.fi" }],
            "role": "air-analyst",
            "scope": { "project": "espoo" }
        }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-analyst-everywhere",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana.kovacova@hel.fi" }],
            "role": "air-analyst",
            "scope": { "organization": "hel" }
        }),
    ));

    let jana = person("jana.kovacova");
    assert!(
        permissions::for_request(&state, &jana, "helsinki").may_read("DataSource"),
        "the role reaches inside its own project"
    );
    assert!(
        !permissions::for_request(&state, &jana, "espoo").may_read("DataSource"),
        "a binding in another project resolves no role of helsinki's (PF-69)"
    );
    // Over the organization the name resolves to nothing at all, so the grant is not in force
    // anywhere else either.
    assert!(
        !permissions::for_request(&state, &jana, "tampere").may_read("DataSource"),
        "an organization-scope binding never reaches a project's own role (PF-69)"
    );
}

/// T-2606, PF-68: Project settings → Roles offers project kinds only, and the door says the same
/// to whoever sends the rest by hand — a project role naming `Role` or `RoleBinding` would let a
/// project write the organization's grants.
#[tokio::test]
async fn a_project_role_naming_an_organization_kind_is_refused_at_the_door() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    // The lead proposes and approves Role and RoleBinding over the whole organization, so the
    // refusal is the kind's place, not their rights.
    for kind in ["Role", "RoleBinding"] {
        let answer = send(
            &state,
            person("lead"),
            "POST",
            "/api/v1/projects/helsinki/roles",
            Some(project_role(
                "helsinki",
                "grant-writer",
                json!([{ "kinds": [kind], "verbs": ["propose"] }]),
            )),
        )
        .await;
        assert!(
            answer.status.is_client_error(),
            "{kind}: {} {}",
            answer.status,
            answer.text
        );
        // The refusal names the kind and the rule: an organization kind is the organization's.
        assert!(
            answer.text.contains(&format!("`{kind}`")) && answer.text.contains("organization"),
            "{kind}: the refusal says why: {}",
            answer.text
        );
    }
}

/// T-2606, PF-69: a project's own role is bound in that project and nowhere else; the binding
/// that names it in another project or over the organization is refused before anything is
/// written, and the one in its own project is the red-lane change it always was.
#[tokio::test]
async fn a_binding_of_a_project_role_outside_its_project_is_refused() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-author",
        "helsinki",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose"] }] }),
    ));

    for (scope, name) in [
        (project("espoo"), "jana-author-espoo"),
        (organization(), "jana-author-everywhere"),
    ] {
        let answer = send(
            &state,
            person("lead"),
            "POST",
            PROPOSE,
            Some(binding(name, "pipeline-author", scope)),
        )
        .await;
        refused(
            &answer,
            "no role pipeline-author is defined where this binding applies",
        );
    }

    let answer = send(
        &state,
        person("lead"),
        "POST",
        PROPOSE,
        Some(binding(
            "jana-author",
            "pipeline-author",
            project("helsinki"),
        )),
    )
    .await;
    red_change(&answer);
}

/// T-2606, PF-59: Project settings reads a project through its existing routes, and a project
/// nobody bound the caller to is not there — 404, never 403, on the project and on its roles.
#[tokio::test]
async fn a_project_the_person_cannot_read_is_not_found_on_its_settings_routes() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    for route in [
        "/api/v1/projects/helsinki",
        "/api/v1/projects/helsinki/roles",
        "/api/v1/projects/helsinki/permissions/me",
    ] {
        let answer = send(&state, person("stranger"), "GET", route, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{route}: {}",
            answer.text
        );
    }
}

#[tokio::test]
async fn a_project_role_is_read_through_the_operations_beside_the_organizations() {
    use joinedcontext_portal::ops::{self, Caller, Via};

    let gitea = forge().await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "air-analyst",
        "helsinki",
        json!({ "rules": [{ "kinds": ["DataSource"], "verbs": ["propose"] }] }),
    ));

    let caller = Caller {
        bearer: None,
        identity: person("admin"),
        via: Via::Mcp,
        access: None,
    };
    let list = ops::find("jc_resource_list").expect("registered");
    let answer = ops::call(list, &caller, &state, "helsinki", json!({ "kind": "Role" }))
        .await
        .expect("the roles in force in helsinki");
    let names: Vec<&str> = answer["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["name"].as_str())
        .collect();
    assert!(names.contains(&"air-analyst"), "{answer}");
    assert!(names.contains(&"org-admin"), "{answer}");

    // Espoo sees the organization's roles and none of helsinki's.
    let answer = ops::call(list, &caller, &state, "espoo", json!({ "kind": "Role" }))
        .await
        .expect("the roles in force in espoo");
    let names: Vec<&str> = answer["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["name"].as_str())
        .collect();
    assert!(!names.contains(&"air-analyst"), "{answer}");
    assert!(names.contains(&"org-admin"), "{answer}");
}

#[tokio::test]
async fn a_project_role_sent_without_a_namespace_is_judged_in_its_project() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    // T-6266: org-admin scoped to the helsinki project, as a project's opener holds it.
    state.mirror.upsert(envelope(
        "RoleBinding",
        "owner-org-admin",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "owner@hel.fi" }], "role": "org-admin",
                "scope": project("helsinki") }),
    ));
    let mut analyst = role(
        "analyst",
        json!([
            { "kinds": ["Endpoint"], "verbs": ["read"] },
            { "kinds": ["Pipeline"], "verbs": ["read", "propose", "approve"] }
        ]),
    );
    analyst["metadata"]
        .as_object_mut()
        .expect("metadata")
        .remove("namespace");
    let roles = "/api/v1/projects/helsinki/roles?dryRun=All";
    let answer = send(
        &state,
        person("owner"),
        "POST",
        roles,
        Some(analyst.clone()),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);

    // The same administrator holds nothing in espoo, so a role there is still refused.
    let answer = send(
        &state,
        person("owner"),
        "POST",
        "/api/v1/projects/espoo/roles?dryRun=All",
        Some(analyst),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
}
