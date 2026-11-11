//! Attack vector T-1684 (PF-52, PF-58): privilege escalation through a grant.
//!
//! PF-52 is a hold on the *proposer*: nobody proposes a `Role`, `RoleBinding` or `ServiceAccount`
//! that grants a verb they do not hold themselves. Every door asks
//! `permissions::within_own_rights` before a Change exists — the resource route, the operations
//! registry, a blueprint, an import — and an approval asks it again. One door did not: bringing a
//! workspace back opened the Change on `effective.check` alone, so the copy a person edits could
//! carry a grant out past the hold every other door applies, and only the approver's own rights
//! stood between it and the repository. That is the first case here.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `access_escalation_tests::a_binding_is_refused_when_its_role_names_a_verb_its_proposer_lacks_on_its_scope`
//! (directly), `::the_operations_registry_refuses_the_same_grant_in_the_same_words` (an
//! operation, which is also the assistant's door — an agent calls `jc_resource_propose` and no
//! other),
//! `::an_approver_approves_a_grant_only_within_their_own_rights_and_a_removal_only_with_delete`
//! (PF-52 at the approval), `::a_service_account_holds_no_role_of_the_organization_its_proposer_lacks`,
//! `::a_project_role_may_not_grant_a_verb_its_proposer_lacks_in_that_project`,
//! `::the_bootstrap_group_grants_the_first_administrator_into_an_empty_repository`,
//! `changes_tests::an_approver_of_bindings_still_may_not_approve_one_that_grants_more_than_they_hold`
//! and `::an_administrator_of_the_kind_approves_their_own_change_and_the_merge_says_so` (PF-58).
//!
//! A `Policy` hands out *data* access on an Endpoint, not a verb of this platform, so no rule of
//! the proposer bounds its operations and `within_own_rights` says nothing about one. Its own
//! gates are the `propose` verb on `Policy` and the publisher role an Endpoint of a public
//! audience needs (EP-76, PF-71), both in `permissions_tests`. What this file does hold for a
//! Policy is PF-58: one that names its author, a Group they are in or a role they hold, and a
//! Group change adding its author, are approved by a second person, an administrator of the kind
//! included (T-5563).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::api::changes::{approve_change_for, ApprovedBy};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";
const WORKSPACE: &str = "escalation";
const BINDING_PATH: &str = "users/assignments/everything-for-me.yaml";

/// A binding that gives `steward@hel.fi` the role `owns-everything`, which grants every verb on
/// every kind across the organization: far above what the steward holds.
fn escalating_binding() -> String {
    "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n  name: \
     everything-for-me\n  namespace: org\nspec:\n  subjects:\n  - user: steward@hel.fi\n  role: \
     owns-everything\n  scope:\n    organization: hel\n"
        .to_owned()
}

/// The steward may read and propose Pipelines in `ovzdusie`, and propose RoleBindings there —
/// enough to reach the hold, and nowhere near enough to pass it. `owns-everything` exists in the
/// organization so the binding names a role that is really there.
fn state_with(shako: &MockServer) -> AppState {
    let state = common::state_on(shako);
    state.mirror.upsert(common::envelope(
        "Role",
        "owns-everything",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["Pipeline", "Endpoint", "Role", "RoleBinding", "ServiceAccount"],
            "verbs": ["read", "propose", "approve", "delete"],
        }] }),
    ));
    state.mirror.upsert(common::envelope(
        "Role",
        "edits-pipelines",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["Pipeline", "RoleBinding"],
            "verbs": ["read", "propose"],
        }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "steward-of-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "steward@hel.fi" }],
            "role": "edits-pipelines",
            "scope": { "project": PROJECT },
        }),
    ));
    state.mirror.upsert(common::envelope(
        "Group",
        "stewards",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "steward@hel.fi" }] }),
    ));
    state
}

/// A forge whose `workspace/escalation` branch carries the escalating binding and whose base and
/// main do not: exactly one file to bring back.
async fn forge_with_workspace() -> MockServer {
    let shako = common::forge().await;
    let branch_head = "ws-head-sha";
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/Branches('workspace%2F{WORKSPACE}')",
            common::REPO
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Name": format!("workspace/{WORKSPACE}"),
            "HeadSha": branch_head
        })))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/Branches('main')", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Name": "main",
            "HeadSha": "main-sha"
        })))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)",
            common::REPO
        )))
        .and(query_param("@r", format!("'workspace/{WORKSPACE}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Tree": [{ "Path": BINDING_PATH, "Type": "blob", "Sha": "blob-binding" }],
            "Truncated": false,
        })))
        .mount(&shako)
        .await;
    for git_ref in ["base-sha", "main"] {
        Mock::given(method("GET"))
            .and(path(format!(
                "{}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)",
                common::REPO
            )))
            .and(query_param("@r", format!("'{git_ref}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Tree": [], "Truncated": false,
            })))
            .mount(&shako)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .and(query_param("@p", format!("'{BINDING_PATH}'")))
        .and(query_param("@r", format!("'workspace/{WORKSPACE}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "blob-binding",
            "Content": common::encode(&escalating_binding())
        })))
        .mount(&shako)
        .await;
    shako
}

/// PF-52, T-1684: a grant above the proposer's own rights does not come back out of a workspace.
/// The refusal names the verb that is missing, and no merge request is opened for it.
#[tokio::test]
async fn a_grant_above_the_proposers_rights_is_not_brought_back_from_a_workspace() {
    let shako = forge_with_workspace().await;
    let state = state_with(&shako);
    state
        .workspaces
        .create(Opening {
            name: WORKSPACE,
            title: None,
            project: PROJECT,
            owner: "steward@hel.fi",
            base_revision: "base-sha",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("a workspace");
    let answer = common::send(
        &state,
        common::person("steward"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/workspaces/{WORKSPACE}/propose"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "a binding granting everything came back out of a workspace: {}",
        answer.text
    );
    assert!(
        answer.text.contains("proposer"),
        "the refusal has to say who is held to what: {}",
        answer.text
    );
    let opened: Vec<String> = shako
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| {
            request.method.to_string().eq_ignore_ascii_case("post")
                && request.url.path().ends_with("/Shako.OpenPullRequest")
        })
        .map(|request| request.url.path().to_owned())
        .collect();
    assert!(
        opened.is_empty(),
        "a merge request was opened for the escalating binding: {opened:?}"
    );
}

/// PF-52: the same binding inside an imported bundle is refused by the same hold, in the same
/// words, before anything is planned.
#[tokio::test]
async fn a_grant_above_the_proposers_rights_is_refused_inside_an_import() {
    let shako = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)",
            common::REPO
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Tree": [],
            "Truncated": false
        })))
        .mount(&shako)
        .await;
    let state = state_with(&shako);
    let answer = common::send(
        &state,
        common::person("steward"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/import?dryRun=All"),
        // The shape `jcctl import` sends: the manifest as the whole body.
        Some(serde_norway::from_str(&escalating_binding()).expect("the binding as JSON")),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "an import carried a binding above the importer's rights: {}",
        answer.text
    );
    assert!(
        answer.text.contains("proposer"),
        "the refusal has to say who is held to what: {}",
        answer.text
    );
}

/// The hold is the *grant*, not the kind: a workspace carrying a RoleBinding the steward's own
/// rights cover comes back as a Change. Without this the case above would pass on a door that
/// simply refused every binding.
#[tokio::test]
async fn a_grant_inside_the_proposers_own_rights_still_comes_back() {
    let shako = forge_with_workspace().await;
    // The same path, a binding to the role the steward holds themselves.
    let within = "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n  name: \
                  everything-for-me\n  namespace: org\nspec:\n  subjects:\n  - group: \
                  stewards\n  role: edits-pipelines\n  scope:\n    project: ovzdusie\n";
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .and(query_param("@p", format!("'{BINDING_PATH}'")))
        .and(query_param("@r", format!("'workspace/{WORKSPACE}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "blob-binding",
            "Content": common::encode(within)
        })))
        .with_priority(1)
        .mount(&shako)
        .await;
    let state = state_with(&shako);
    state
        .workspaces
        .create(Opening {
            name: WORKSPACE,
            title: None,
            project: PROJECT,
            owner: "steward@hel.fi",
            base_revision: "base-sha",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("a workspace");
    let answer = common::send(
        &state,
        common::person("steward"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/workspaces/{WORKSPACE}/propose"),
        Some(json!({})),
    )
    .await;
    assert_ne!(
        answer.status,
        StatusCode::FORBIDDEN,
        "a grant the steward holds themselves was refused: {}",
        answer.text
    );
}

/// The refusal carries nothing of the manifest it refused: a binding is a `users/` file and a
/// person reading the answer learns which verb is missing, not what else the file said.
#[tokio::test]
async fn the_refusal_names_the_verb_and_carries_nothing_else_of_the_manifest() {
    let shako = forge_with_workspace().await;
    let state = state_with(&shako);
    state
        .workspaces
        .create(Opening {
            name: WORKSPACE,
            title: None,
            project: PROJECT,
            owner: "steward@hel.fi",
            base_revision: "base-sha",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("a workspace");
    let answer = common::send(
        &state,
        common::person("steward"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/workspaces/{WORKSPACE}/propose"),
        Some(json!({})),
    )
    .await;
    let body: Value = serde_json::from_str(&answer.text).expect("a problem document");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("owns-everything") || detail.contains("proposer"),
        "the refusal names the role or the proposer: {detail}"
    );
    assert!(
        !detail.contains("steward@hel.fi"),
        "the refusal repeated the subject of the binding back: {detail}"
    );
}

// --- T-5563: nobody grants themselves access alone (PF-58) ---

const AUTHOR: &str = "steward@hel.fi";

/// A forge holding one merge request by `steward@hel.fi` that adds `file` with `yaml`, and a
/// mirror where the steward and `peer@hel.fi` both administer Policy and Group across the
/// organization: the case where only the self-grant rule stands between the steward and a merge.
async fn self_grant(file: &str, yaml: &str) -> (MockServer, AppState) {
    let kind = if file.starts_with("users/groups/") {
        "group"
    } else {
        "policy"
    };
    let shako = common::forge().await;
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", common::REPO)))
        .respond_with(ok(
            json!({ "Sha": "blob-1", "Content": common::encode(yaml) }),
        ))
        .with_priority(1)
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/PullRequests(1)", common::REPO)))
        .respond_with(ok(json!({
            "Number": 1,
            "HtmlUrl": "https://shako.example.sk/pulls/1",
            "State": "open",
            "Title": "create a grant",
            "Head": format!("portal/create-{kind}-mine-11111111"),
            "HeadSha": "head-1",
            "Base": "main",
            "CreatedAt": "2026-10-25T09:14:22Z",
            "Author": "steward",
            "AuthorName": "steward",
            "AuthorEmail": AUTHOR,
            "Mergeable": true,
            "Merged": false
        })))
        .with_priority(1)
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/PullRequests(1)/Files", common::REPO)))
        .respond_with(ok(
            json!({ "value": [{ "Filename": file, "Status": "added" }] }),
        ))
        .with_priority(1)
        .mount(&shako)
        .await;
    let state = common::state_on(&shako);
    state.mirror.upsert(common::envelope(
        "Role",
        "grants-access",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["Policy", "Group", "ContextSpace"],
            "verbs": ["read", "propose", "approve", "delete"],
        }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "grants-access",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": AUTHOR }, { "user": "peer@hel.fi" }],
            "role": "grants-access",
            "scope": { "organization": "hel" }
        }),
    ));
    state.mirror.upsert(common::envelope(
        "Group",
        "air-team",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": AUTHOR }] }),
    ));
    (shako, state)
}

fn policy(rules: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: mine\n  \
         namespace: {PROJECT}\nspec:\n  contextSpaceRef: mobility\n  assigner: \
         did:web:hel.fi\n{rules}"
    )
}

const POLICY_PATH: &str = "projects/ovzdusie/spaces/mobility/policies/mine.yaml";

async fn approve(state: &AppState, who: Identity) -> Result<(), ApiError> {
    approve_change_for(
        state,
        &who,
        ORG_NAMESPACE,
        "chg-org-00000001",
        Some("mine"),
        ApprovedBy::Person,
    )
    .await
    .map(drop)
}

async fn merged(shako: &MockServer) -> bool {
    shako
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .any(|r| {
            r.method.as_str() == "POST" && r.url.path().ends_with("/PullRequests(1)/Shako.Merge")
        })
}

/// The author's own approval is refused with the rule's words, nothing merges, and a peer's
/// approval of the same change goes through.
async fn waits_for_another_person(file: &str, yaml: &str, author: Identity) {
    let (shako, state) = self_grant(file, yaml).await;
    let err = approve(&state, author)
        .await
        .expect_err("approved by its author");
    assert!(matches!(err, ApiError::SelfApproval(_)), "{err:?}");
    assert!(
        err.to_string().contains("grants access to its own author")
            && err.to_string().contains("PF-58"),
        "{err}"
    );
    assert!(!merged(&shako).await, "merged by its author");

    let (shako, state) = self_grant(file, yaml).await;
    approve(&state, common::person("peer"))
        .await
        .unwrap_or_else(|err| panic!("a peer approves: {err:?}"));
    assert!(merged(&shako).await, "a peer's approval did not merge");
}

#[tokio::test]
async fn a_policy_naming_its_author_waits_for_another_person() {
    let yaml = policy(&format!(
        "  assignee:\n    kind: user\n    id: {AUTHOR}\n  operations: [read, write, delete]\n"
    ));
    waits_for_another_person(POLICY_PATH, &yaml, common::person("steward")).await;
}

#[tokio::test]
async fn a_policy_naming_a_group_of_its_author_waits_for_another_person() {
    let yaml =
        policy("  assignee:\n    kind: group\n    id: air-team\n  operations: [read, write]\n");
    waits_for_another_person(POLICY_PATH, &yaml, common::person("steward")).await;
}

#[tokio::test]
async fn a_group_change_adding_its_author_waits_for_another_person() {
    let yaml = format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Group\nmetadata:\n  name: mine\n  \
         namespace: {ORG_NAMESPACE}\nspec:\n  members:\n  - user: {AUTHOR}\n"
    );
    waits_for_another_person("users/groups/mine.yaml", &yaml, common::person("steward")).await;
}

/// The administrator of Policy who holds the realm role a rule of the Policy names: their own
/// approval would have merged before T-5563 (PF-58 let an administrator of the kind approve their
/// own change), and now waits like anyone's.
#[tokio::test]
async fn an_administrator_cannot_approve_their_own_self_grant() {
    let yaml = policy(
        "  rules:\n  - effect: prohibition\n    assignee:\n      kind: user\n      id: someone@hel.fi\n    \
         operations: [delete]\n  - assignee:\n      kind: role\n      id: air-admins\n    operations: [read]\n",
    );
    let mut author = common::person("steward");
    author.roles.push("air-admins".to_owned());
    waits_for_another_person(POLICY_PATH, &yaml, author).await;
}

/// A Policy for someone else is still access: its administrator author waits for a second
/// person like any other access change, and a peer's approval merges (P-10, T-6714).
#[tokio::test]
async fn a_policy_for_someone_else_waits_for_a_second_person() {
    let yaml =
        policy("  assignee:\n    kind: user\n    id: someone@hel.fi\n  operations: [read]\n");
    let (shako, state) = self_grant(POLICY_PATH, &yaml).await;
    let err = approve(&state, common::person("steward"))
        .await
        .expect_err("approved by its author");
    assert!(matches!(err, ApiError::SelfApproval(_)), "{err:?}");
    assert!(
        err.to_string().contains("a change to Policy is approved by a second administrator"),
        "{err}"
    );
    assert!(!merged(&shako).await, "merged by its author");

    let (shako, state) = self_grant(POLICY_PATH, &yaml).await;
    approve(&state, common::person("peer"))
        .await
        .unwrap_or_else(|err| panic!("a peer approves: {err:?}"));
    assert!(merged(&shako).await, "a peer's approval did not merge");
}
