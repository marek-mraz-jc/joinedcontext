//! An App's Endpoint and Policies ride in the change that proposes the App (CC-61, AP-96,
//! T-2632). The gateway reads endpoints and policies from the configuration repository alone, so
//! a role-gated write nobody commits is a write the gateway never refuses or allows by role.
//!
//! The door commits the rendered grants with the App, keeps the endpoint's slug across
//! republishing, removes the grants a retired App no longer holds, lists every grant in the plan,
//! and refuses to overwrite an Endpoint of the same name somebody wrote by hand.

mod common;

use axum::http::StatusCode;
use base64::Engine;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{checked_send as send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, API_VERSION};
use joinedcontext_portal::state::AppState;

const APPS: &str = "/api/v1/projects/helsinki/apps";
const GENERATED: &str = "portal/app-reconciler";

/// The steward an App names by Keycloak user id (ADR-N-061, PF-126).
const JANA_ID: &str = "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14";

fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose", "read", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-app-editor",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "app-editor",
            "scope": { "project": "helsinki" },
        }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "bikes",
        "helsinki",
        json!({ "dataModelRef": "bikes" }),
    ));
    state
}

fn app(lifecycle: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": { "name": "bikes", "namespace": "helsinki" },
        "spec": {
            "kind": "static",
            "source": { "git": {
                "url": "https://git.example/joinedcontext/helsinki_bikes.git",
                "ref": "0123456789abcdef0123456789abcdef01234567",
            }},
            "build": { "node": "22" },
            "visibility": "roles",
            "lifecycle": lifecycle,
            "roles": [{ "name": "viewer" }, { "name": "steward" }],
            "access": [
                { "role": "viewer", "subjects": [{ "group": "bikes-readers" }] },
                { "role": "steward", "subjects": [{ "user": JANA_ID }] },
            ],
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
                    "types": ["BikeHireDockingStation"],
                    "operations": ["queryEntity"],
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
                    "types": ["BikeHireDockingStation"],
                    "attrs": ["stewardNote"],
                    "operations": ["updateAttrs"],
                    "roles": ["steward"],
                },
            ],
        },
    })
}

fn generated(
    kind: &str,
    name: &str,
    spec: Value,
) -> joinedcontext_portal::resource::ResourceEnvelope {
    let mut metadata = ObjectMeta::new(name, "helsinki");
    metadata.annotations.insert(
        "joinedcontext.com/generated-by".to_owned(),
        GENERATED.to_owned(),
    );
    joinedcontext_portal::resource::ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata,
        spec,
        status: None,
    }
}

/// Every file of every multi-file commit the forge received: `(operation, path, yaml)`.
async fn committed(gitea: &MockServer) -> Vec<(String, String, String)> {
    let mut files = Vec::new();
    for request in gitea.received_requests().await.unwrap_or_default() {
        if request.method.as_str() != "POST"
            || request.url.path() != format!("{}/contents", common::REPO)
        {
            continue;
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        for file in body["files"].as_array().into_iter().flatten() {
            let content = file["content"]
                .as_str()
                .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            files.push((
                file["operation"].as_str().unwrap_or_default().to_owned(),
                file["path"].as_str().unwrap_or_default().to_owned(),
                content,
            ));
        }
    }
    files
}

#[tokio::test]
async fn a_published_role_gated_app_commits_its_endpoint_and_one_policy_per_role() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let accepted = send(&state, person("jana"), "POST", APPS, Some(app("published"))).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);

    let files = committed(&gitea).await;
    let endpoint = files
        .iter()
        .find(|(op, path, _)| {
            op == "upload" && path.contains("endpoint") && path.ends_with("app-bikes.yaml")
        })
        .unwrap_or_else(|| panic!("the App's Endpoint is committed: {files:?}"));
    assert!(endpoint.2.contains("callerRole: true"), "{}", endpoint.2);
    assert!(
        endpoint.2.contains(JANA_ID),
        "the steward's subject: {}",
        endpoint.2
    );
    assert!(endpoint.2.contains("slug:"), "{}", endpoint.2);

    let policies: Vec<&String> = files
        .iter()
        .filter(|(op, path, _)| op == "upload" && path.contains("polic"))
        .map(|(_, path, _)| path)
        .collect();
    assert_eq!(
        policies.len(),
        2,
        "a read for the endpoint, a write for the steward: {policies:?}"
    );
    assert!(
        policies.iter().any(|p| p.ends_with("app-bikes-1.yaml")),
        "{policies:?}"
    );
    assert!(
        policies
            .iter()
            .any(|p| p.ends_with("app-bikes-2-steward.yaml")),
        "{policies:?}"
    );
    let steward = &files
        .iter()
        .find(|(_, p, _)| p.ends_with("app-bikes-2-steward.yaml"))
        .map(|f| &f.2);
    assert!(
        steward.is_some_and(|yaml| yaml.contains("endpoint:helsinki/app-bikes/steward")
            && yaml.contains("updateAttrs")),
        "{steward:?}"
    );

    // The reviewer reads who may do what in the check before the proposal (AP-98).
    let checked = send(
        &state,
        person("jana"),
        "POST",
        &format!("{APPS}?dryRun=All"),
        Some(app("published")),
    )
    .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text);
    for said in [
        "role steward of bikes can updateAttrs BikeHireDockingStation",
        "everyone who can open bikes can queryEntity",
    ] {
        assert!(checked.text.contains(said), "{said}: {}", checked.text);
    }
}

#[tokio::test]
async fn republishing_keeps_the_endpoints_slug() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    state.mirror.upsert(generated(
        "Endpoint",
        "app-bikes",
        json!({ "slug": "kept2slug3of4the5app6endpoint7", "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" } }),
    ));

    let accepted = send(&state, person("jana"), "POST", APPS, Some(app("published"))).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let files = committed(&gitea).await;
    let endpoint = files
        .iter()
        .find(|(_, p, _)| p.ends_with("app-bikes.yaml"))
        .map(|f| f.2.clone())
        .unwrap_or_default();
    assert!(
        endpoint.contains("kept2slug3of4the5app6endpoint7"),
        "{endpoint}"
    );
}

#[tokio::test]
async fn a_retired_app_removes_its_grants_and_leaves_another_apps_alone() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/contents/.*\\.yaml$", common::REPO)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "blob-1", "content": "" })),
        )
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    state.mirror.upsert(generated(
        "Endpoint",
        "app-bikes",
        json!({ "slug": "kept2slug3of4the5app6endpoint7", "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" } }),
    ));
    let grant = |id: &str| {
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
            "assignee": { "kind": "role", "id": id },
            "operations": ["queryEntity"],
        })
    };
    state.mirror.upsert(generated(
        "Policy",
        "app-bikes-2-steward",
        grant("endpoint:helsinki/app-bikes/steward"),
    ));
    // App `bikes-2`'s first grant: its name starts like one of `bikes`'s, its assignee does not.
    state.mirror.upsert(generated(
        "Policy",
        "app-bikes-2-1",
        grant("endpoint:helsinki/app-bikes-2"),
    ));

    let accepted = send(&state, person("jana"), "POST", APPS, Some(app("retired"))).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let files = committed(&gitea).await;
    let deleted: Vec<&String> = files
        .iter()
        .filter(|(op, _, _)| op == "delete")
        .map(|(_, p, _)| p)
        .collect();
    assert!(
        deleted.iter().any(|p| p.ends_with("app-bikes.yaml")),
        "{deleted:?}"
    );
    assert!(
        deleted
            .iter()
            .any(|p| p.ends_with("app-bikes-2-steward.yaml")),
        "{deleted:?}"
    );
    assert!(
        !deleted.iter().any(|p| p.ends_with("app-bikes-2-1.yaml")),
        "another App's grant: {deleted:?}"
    );
    assert!(
        !files.iter().any(|(op, _, _)| op == "upload"),
        "a retired App grants nothing: {files:?}"
    );
}

#[tokio::test]
async fn a_hand_written_endpoint_with_the_apps_name_is_refused_not_overwritten() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "Endpoint",
        "app-bikes",
        "helsinki",
        json!({ "slug": "h4nd3wr1tt3n0001", "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" } }),
    ));

    let refused = send(&state, person("jana"), "POST", APPS, Some(app("published"))).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("app-bikes"), "{}", refused.text);
    assert!(committed(&gitea).await.is_empty(), "nothing was committed");
}

/// The assigner of every committed Policy: one `did:web:` line per Policy file.
fn assigners(files: &[(String, String, String)]) -> Vec<String> {
    files
        .iter()
        .filter(|(op, path, _)| op == "upload" && path.contains("polic"))
        .flat_map(|(_, _, yaml)| {
            yaml.lines()
                .filter_map(|line| line.trim().strip_prefix("assigner: "))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// PF-108, T-4375: an App's Policies name its project's publisher as their assigner, the
/// Project's `spec.domain`; a project that names none falls back to the organization's domain.
#[tokio::test]
async fn an_apps_policies_name_the_projects_publisher_else_the_organization() {
    for domain in [Some("kaupunki.hel.fi"), None] {
        let gitea = forge().await;
        let state = state_with(&gitea);
        let mut spec = json!({ "organizationRef": { "kind": "Organization", "name": "helsinki" } });
        if let Some(domain) = domain {
            spec["domain"] = json!(domain);
        }
        state
            .mirror
            .upsert(envelope("Project", "helsinki", ORG_NAMESPACE, spec));
        let want = format!(
            "did:web:{}",
            domain.map_or_else(
                || joinedcontext_portal::api::assistant::org_domain(&state, "helsinki"),
                str::to_owned
            )
        );

        let accepted = send(&state, person("jana"), "POST", APPS, Some(app("published"))).await;
        assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
        let found = assigners(&committed(&gitea).await);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|a| *a == want), "{want}: {found:?}");
    }
}

#[tokio::test]
async fn deleting_an_app_removes_its_endpoint_and_policies_in_the_same_commit() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/contents/.*\\.yaml$", common::REPO)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "blob-1", "content": "" })),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/git/trees/.*", common::REPO)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": [], "truncated": false })),
        )
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "App",
        "bikes",
        "helsinki",
        app("published")["spec"].clone(),
    ));
    state.mirror.upsert(generated(
        "Endpoint",
        "app-bikes",
        json!({ "slug": "kept2slug3of4the5app6endpoint7", "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" } }),
    ));
    state.mirror.upsert(generated(
        "Policy",
        "app-bikes-2-steward",
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
            "assignee": { "kind": "role", "id": "endpoint:helsinki/app-bikes/steward" },
            "operations": ["updateAttrs"],
        }),
    ));

    let removed = send(
        &state,
        person("jana"),
        "DELETE",
        &format!("{APPS}/bikes"),
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::ACCEPTED, "{}", removed.text);
    let deleted: Vec<String> = committed(&gitea)
        .await
        .into_iter()
        .filter(|(op, _, _)| op == "delete")
        .map(|(_, path, _)| path)
        .collect();
    for file in [
        "apps/bikes/app.yaml",
        "/app-bikes.yaml",
        "/app-bikes-2-steward.yaml",
    ] {
        assert!(
            deleted.iter().any(|p| p.ends_with(file)),
            "{file}: {deleted:?}"
        );
    }
}

/// The org Group `name`, owned by App `bikes` of `helsinki` when `owned`, with `members`.
fn group(
    name: &str,
    owned: bool,
    members: &[&str],
) -> joinedcontext_portal::resource::ResourceEnvelope {
    let mut group = envelope(
        "Group",
        name,
        ORG_NAMESPACE,
        json!({ "members": members.iter().map(|m| json!({ "user": m })).collect::<Vec<_>>() }),
    );
    if owned {
        group.metadata.annotations.insert(
            "joinedcontext.com/app".to_owned(),
            "helsinki/bikes".to_owned(),
        );
    }
    group
}

/// The dry run's plan as `path → to` lines.
fn planned(text: &str) -> Vec<(String, Value)> {
    let body: Value = serde_json::from_str(text).unwrap_or_default();
    body["plan"]["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|field| {
            (
                field["path"].as_str().unwrap_or_default().to_owned(),
                field["to"].clone(),
            )
        })
        .collect()
}

/// AP-118: each role of a proposed App gets its empty default group, annotated with the App, in
/// the same commit as its grants, and the access entry giving it the role; the plan names both.
#[tokio::test]
async fn every_role_of_a_proposed_app_commits_its_default_group() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let checked = send(
        &state,
        person("jana"),
        "POST",
        &format!("{APPS}?dryRun=All"),
        Some(app("published")),
    )
    .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text);
    let fields = planned(&checked.text);
    for group in ["groups.bikes-viewer", "groups.bikes-steward"] {
        assert!(
            fields.iter().any(|(path, _)| path == group),
            "{group}: {fields:?}"
        );
    }

    let accepted = send(&state, person("jana"), "POST", APPS, Some(app("published"))).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let files = committed(&gitea).await;
    for name in ["bikes-viewer", "bikes-steward"] {
        let (_, _, yaml) = files
            .iter()
            .find(|(op, path, _)| op == "upload" && path == &format!("users/groups/{name}.yaml"))
            .unwrap_or_else(|| panic!("{name} is committed: {files:?}"));
        assert!(
            yaml.contains("joinedcontext.com/app: helsinki/bikes") && yaml.contains("members: []"),
            "{yaml}"
        );
    }
    // The endpoint gives each role to its group, beside whoever the author named.
    let endpoint = files
        .iter()
        .find(|(_, path, _)| path.ends_with("app-bikes.yaml"))
        .map(|(_, _, yaml)| yaml.clone())
        .unwrap_or_default();
    for subject in ["bikes-viewer", "bikes-steward", "bikes-readers", JANA_ID] {
        assert!(endpoint.contains(subject), "{subject}: {endpoint}");
    }
}

/// AP-118: a role taken away removes its group in the same commit, warns that its members lose
/// the role, and the App names the group nowhere; a group of another owner is never touched.
#[tokio::test]
async fn a_role_taken_away_removes_its_group_and_warns_about_its_members() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/contents/.*\\.yaml$", common::REPO)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "blob-1", "content": "" })),
        )
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    state
        .mirror
        .upsert(group("bikes-viewer", true, &["ada@hel.fi", "bo@hel.fi"]));
    state.mirror.upsert(group("bikes-steward", true, &[]));
    state
        .mirror
        .upsert(group("bikes-readers", false, &["cy@hel.fi"]));

    let mut only_steward = app("published");
    only_steward["spec"]["roles"] = json!([{ "name": "steward" }]);
    only_steward["spec"]["access"] = json!([
        { "role": "steward", "subjects": [{ "group": "bikes-steward" }] },
    ]);

    let checked = send(
        &state,
        person("jana"),
        "POST",
        &format!("{APPS}?dryRun=All"),
        Some(only_steward.clone()),
    )
    .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text);
    let fields = planned(&checked.text);
    assert!(
        fields
            .iter()
            .any(|(path, to)| path.starts_with("warnings.groups.")
                && to.as_str().is_some_and(
                    |said| said.contains("'bikes-viewer'") && said.contains("2 member(s)")
                )),
        "{fields:?}"
    );

    let accepted = send(&state, person("jana"), "POST", APPS, Some(only_steward)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let files = committed(&gitea).await;
    let deleted: Vec<&String> = files
        .iter()
        .filter(|(op, _, _)| op == "delete")
        .map(|(_, path, _)| path)
        .collect();
    assert!(
        deleted.contains(&&"users/groups/bikes-viewer.yaml".to_owned()),
        "{deleted:?}"
    );
    assert!(
        !deleted
            .iter()
            .any(|p| p.contains("bikes-steward") || p.contains("bikes-readers")),
        "{deleted:?}"
    );
    assert!(
        !files
            .iter()
            .any(|(op, _, yaml)| op == "upload" && yaml.contains("bikes-viewer")),
        "nothing committed names the removed group: {files:?}"
    );
}

/// AP-115: a default group whose name the organization's own group holds is refused at the
/// door, naming both owners, and nothing reaches the forge.
#[tokio::test]
async fn a_default_group_named_like_the_organizations_group_is_refused() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    state
        .mirror
        .upsert(group("bikes-viewer", false, &["ada@hel.fi"]));

    let refused = send(&state, person("jana"), "POST", APPS, Some(app("published"))).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        refused.text.contains("belongs to the organization")
            && refused.text.contains("the App bikes of project helsinki"),
        "{}",
        refused.text
    );
    assert!(committed(&gitea).await.is_empty());
}

/// AP-118 at the import door: an imported App's roles come with their default groups, listed as
/// created, and the App gives each role to its group.
#[tokio::test]
async fn an_imported_app_brings_its_default_groups() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/git/trees/.*", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tree": [], "truncated": false,
        })))
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    state.mirror.upsert(group("bikes-steward", true, &[]));

    let checked = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/import?dryRun=All",
        Some(app("published")),
    )
    .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text);
    let body: Value = serde_json::from_str(&checked.text).unwrap_or_default();
    let created = body["created"].as_array().cloned().unwrap_or_default();
    assert!(created.contains(&json!("bikes-viewer")), "{created:?}");
    assert!(
        !created.contains(&json!("bikes-steward")),
        "a group already there is not created again: {created:?}"
    );
}
