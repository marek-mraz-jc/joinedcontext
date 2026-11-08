//! The administration operations per persona (owner order 2026-11-08, MCP-ADMIN.md): a platform
//! administrator (org-admin of the installation Organization `hel`), an organization
//! administrator (org-admin of `esp`, ADR-N-058 §2.4) and a project administrator (steward of
//! `esp-air`) each see and run only what their role lets them, through the MCP door, the REST
//! door and the session, and nobody reads across Organizations.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::doors::{call_in, mcp, mcp_rpc, run, session};
use common::envelope;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::{self, administration, Caller};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const HEL_PROJECT: &str = "hel-bus";
const ESP_PROJECT: &str = "esp-air";

fn someone(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

/// The same person through their own MCP client, whose token names the client.
fn through_client(identity: Identity) -> Identity {
    Identity {
        client: Some("mcp-local".into()),
        ..identity
    }
}

fn platform() -> Identity {
    someone("plat")
}

fn organization_admin() -> Identity {
    someone("orga")
}

fn project_admin() -> Identity {
    someone("proj")
}

/// A project administrator of the installation's Organization, the one `esp`'s people never see.
fn other_project_admin() -> Identity {
    someone("hela")
}

fn binding(
    name: &str,
    user: &str,
    role: &str,
    scope: Value,
) -> joinedcontext_portal::resource::ResourceEnvelope {
    envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{user}@example.org") }], "role": role, "scope": scope }),
    )
}

fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    m.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "domain": "hel.fi", "installation": true }),
    ));
    m.upsert(envelope(
        "Organization",
        "esp",
        ORG_NAMESPACE,
        json!({ "domain": "espoo.fi" }),
    ));
    for (project, org) in [(HEL_PROJECT, "hel"), (ESP_PROJECT, "esp")] {
        m.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({ "organizationRef": org }),
        ));
        m.upsert(envelope(
            "ServiceAccount",
            &format!("{project}-cam"),
            project,
            json!({ "owner": { "user": "proj@example.org" }, "purpose": "a camera posting counts", "roles": [], "credentials": [{ "kind": "oauth-client", "name": "main" }] }),
        ));
    }
    // A rule names its kinds one by one (jc-core `Rule::grants`): every kind the Portal has.
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    let every =
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }] });
    m.upsert(envelope("Role", "org-admin", ORG_NAMESPACE, every.clone()));
    m.upsert(envelope("Role", "steward", ORG_NAMESPACE, every));
    m.upsert(binding(
        "platform",
        "plat",
        "org-admin",
        json!({ "organization": "hel" }),
    ));
    m.upsert(binding(
        "esp-admins",
        "orga",
        "org-admin",
        json!({ "organization": "esp" }),
    ));
    m.upsert(binding(
        "esp-air-steward",
        "proj",
        "steward",
        json!({ "project": ESP_PROJECT }),
    ));
    m.upsert(binding(
        "hel-bus-steward",
        "hela",
        "steward",
        json!({ "project": HEL_PROJECT }),
    ));
    state
}

/// The administration operations `caller` is offered in `project` at `tools/list`.
async fn listed(caller: &Caller, state: &AppState, project: &str) -> BTreeSet<String> {
    let answer = mcp_rpc(
        caller,
        state,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "project": project } }),
    )
    .await;
    answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools: {answer}"))
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .filter(|name| administration::NAMES.contains(name))
        .map(str::to_owned)
        .collect()
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

#[tokio::test]
async fn each_persona_is_listed_exactly_its_administration_tools() {
    let state = world();
    let all = names(administration::NAMES);
    let none = BTreeSet::new();
    for (who, project, expected) in [
        (platform(), ORG_NAMESPACE, &all),
        (platform(), HEL_PROJECT, &all),
        // Another Organization's project is that Organization's own, deny by default (ADR-N-058
        // §2.4): the installation's administrator governs the installation, not its data.
        (platform(), ESP_PROJECT, &none),
        (organization_admin(), ESP_PROJECT, &all),
        // The installation's own namespace and projects are not `esp`'s (ADR-N-058 §2.4).
        (organization_admin(), HEL_PROJECT, &none),
        (project_admin(), ESP_PROJECT, &all),
        (project_admin(), HEL_PROJECT, &none),
        (project_admin(), ORG_NAMESPACE, &none),
        (other_project_admin(), ESP_PROJECT, &none),
        (someone("nobody"), ESP_PROJECT, &none),
    ] {
        let caller = mcp(through_client(who.clone()));
        assert_eq!(
            &listed(&caller, &state, project).await,
            expected,
            "{} in {project}",
            who.username
        );
    }
}

#[tokio::test]
async fn an_agent_run_is_never_offered_nor_runs_an_administration_tool() {
    let state = world();
    let caller = run(platform(), administration::NAMES);
    let offered: Vec<String> = ops::listing(&caller, &state, HEL_PROJECT)
        .into_iter()
        .map(|op| op.name)
        .filter(|name| administration::NAMES.contains(&name.as_str()))
        .collect();
    assert!(offered.is_empty(), "{offered:?}");
    let answer = call_in(
        administration::ORGANIZATION,
        &caller,
        &state,
        HEL_PROJECT,
        json!({ "aspect": "projects" }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text());
    assert!(answer.text().contains("AG-11"), "{}", answer.text());
}

fn project_names(answer: &Value) -> BTreeSet<String> {
    answer["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {answer}"))
        .iter()
        .filter_map(|item| item["name"].as_str())
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn the_project_list_never_names_another_organizations_project() {
    let state = world();
    let read = |who: Identity, project: &'static str| {
        let state = state.clone();
        async move {
            call_in(
                administration::ORGANIZATION,
                &mcp(through_client(who)),
                &state,
                project,
                json!({ "aspect": "projects" }),
            )
            .await
        }
    };
    for (who, own, foreign) in [
        (platform(), HEL_PROJECT, ESP_PROJECT),
        (organization_admin(), ESP_PROJECT, HEL_PROJECT),
        (project_admin(), ESP_PROJECT, HEL_PROJECT),
        (other_project_admin(), HEL_PROJECT, ESP_PROJECT),
    ] {
        let answer = read(who.clone(), own).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text());
        let seen = project_names(&answer.body);
        assert!(seen.contains(own), "{}: {seen:?}", who.username);
        assert!(
            !seen.contains(foreign),
            "{} reads {foreign}: {seen:?}",
            who.username
        );
        assert!(
            !answer.text().contains(foreign),
            "{} learns of {foreign}: {}",
            who.username,
            answer.text()
        );
        // Asked inside the other Organization's project, the project is not there (PF-59, R20).
        let refused = read(who.clone(), foreign).await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text());
    }
}

#[tokio::test]
async fn the_organization_readings_are_for_the_installations_administrators() {
    let state = world();
    for aspect in ["setup", "health", "healthChecks", "brokers"] {
        let input = json!({ "aspect": aspect });
        let answer = call_in(
            administration::ORGANIZATION,
            &session(platform()),
            &state,
            ORG_NAMESPACE,
            input.clone(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::OK, "{aspect}: {}", answer.text());
        assert!(answer.body.is_object(), "{aspect}: {}", answer.text());
        for (who, project) in [
            (organization_admin(), ESP_PROJECT),
            (project_admin(), ESP_PROJECT),
        ] {
            let refused = call_in(
                administration::ORGANIZATION,
                &mcp(through_client(who.clone())),
                &state,
                project,
                input.clone(),
            )
            .await;
            assert_eq!(
                refused.status,
                StatusCode::FORBIDDEN,
                "{aspect} for {}: {}",
                who.username,
                refused.text()
            );
        }
    }
}

#[tokio::test]
async fn the_rest_door_answers_what_the_operation_answers() {
    let state = world();
    let uri = format!(
        "/api/v1/projects/{ORG_NAMESPACE}/ops/{}",
        administration::ORGANIZATION
    );
    let page = common::send(
        &state,
        platform(),
        "GET",
        "/api/v1/organization/setup",
        None,
    )
    .await;
    let tool = common::send(
        &state,
        platform(),
        "POST",
        &uri,
        Some(json!({ "aspect": "setup" })),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    assert_eq!(tool.status, StatusCode::OK, "{}", tool.text);
    let page: Value = serde_json::from_str(&page.text).expect("json");
    let tool: Value = serde_json::from_str(&tool.text).expect("json");
    assert_eq!(page, tool);
    let refused = common::send(
        &state,
        platform(),
        "POST",
        &uri,
        Some(json!({ "aspect": "setup", "limit": 3 })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(refused.text.contains("limit"), "{}", refused.text);
}

#[tokio::test]
async fn a_service_accounts_access_is_read_in_its_own_project_only() {
    let state = world();
    let read = |who: Identity, project: &'static str, name: &'static str| {
        let state = state.clone();
        async move {
            call_in(
                administration::ACCOUNT,
                &mcp(through_client(who)),
                &state,
                project,
                json!({ "name": name }),
            )
            .await
        }
    };
    let own = read(project_admin(), ESP_PROJECT, "esp-air-cam").await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text());
    let own = read(organization_admin(), ESP_PROJECT, "esp-air-cam").await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text());
    let foreign = read(project_admin(), HEL_PROJECT, "hel-bus-cam").await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{}", foreign.text());
    let foreign = read(organization_admin(), HEL_PROJECT, "hel-bus-cam").await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{}", foreign.text());
    // Named from inside its own project, another project's account is not there either.
    let across = read(project_admin(), ESP_PROJECT, "hel-bus-cam").await;
    assert_eq!(across.status, StatusCode::NOT_FOUND, "{}", across.text());
}

#[tokio::test]
async fn an_mcp_call_of_an_administration_tool_is_one_row_on_the_activity() {
    let state = world();
    let caller = mcp(through_client(project_admin()));
    let answer = mcp_rpc(
        &caller,
        &state,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": administration::ACCOUNT,
            "arguments": { "project": ESP_PROJECT, "name": "esp-air-cam" } } }),
    )
    .await;
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    assert!(
        answer["result"]["structuredContent"].is_object(),
        "{answer}"
    );
    let filter = joinedcontext_portal::activity::ActivityFilter {
        limit: 50,
        ..Default::default()
    };
    let page = state
        .activity
        .list(ESP_PROJECT, &filter)
        .await
        .expect("activity");
    let rows = serde_json::to_string(&page.items).expect("json");
    assert!(rows.contains(administration::ACCOUNT), "{rows}");
}

#[tokio::test]
async fn access_requests_need_the_portals_database_and_a_decision_says_why() {
    let state = world();
    let listed = call_in(
        administration::REQUESTS,
        &mcp(through_client(project_admin())),
        &state,
        ESP_PROJECT,
        json!({}),
    )
    .await;
    // The incoming list is the mirror's, so it answers without a database: nothing pending.
    assert!(
        listed.status == StatusCode::OK || listed.status == StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        listed.text()
    );
    let refused = call_in(
        administration::DECIDE,
        &mcp(through_client(project_admin())),
        &state,
        ESP_PROJECT,
        json!({ "id": "r-1", "decision": "revoke" }),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text()
    );
    let stranger = call_in(
        administration::DECIDE,
        &mcp(through_client(other_project_admin())),
        &state,
        ESP_PROJECT,
        json!({ "id": "r-1", "decision": "approve" }),
    )
    .await;
    assert_eq!(
        stranger.status,
        StatusCode::FORBIDDEN,
        "{}",
        stranger.text()
    );
}

// ---------------------------------------------------------------------------------------------
// One scripted setup per persona: Organization, project, space, Endpoint and Pipeline through
// the registry door an MCP `tools/call` reaches once the person confirmed (AG-63). Each Change a
// step opens is approved by a person in the Portal (AG-133); the scenario stands in for that by
// putting the approved manifest into the mirror, as the reconciler does after the merge.
// ---------------------------------------------------------------------------------------------

async fn scenario_world() -> AppState {
    let state = common::state_on_forge().await;
    let world = world();
    for kind in ["Organization", "Project", "Role", "RoleBinding"] {
        for item in world
            .mirror
            .list(ORG_NAMESPACE, kind, &Default::default())
            .items
        {
            state.mirror.upsert(item);
        }
    }
    state
}

/// What a step's door answered: the status and the body. A proposal is checked first with the
/// same manifest, as every door asks since T-0956 (PF-57).
async fn step(
    who: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    if name.ends_with("_propose") {
        let check = json!({ "manifest": input["manifest"] });
        let checked = call_in(
            "jc_manifest_dry_run",
            &mcp(through_client(who.clone())),
            state,
            project,
            check,
        )
        .await;
        assert_eq!(
            checked.status,
            StatusCode::OK,
            "the check: {}",
            checked.text()
        );
    }
    let answer = call_in(
        name,
        &mcp(through_client(who.clone())),
        state,
        project,
        input,
    )
    .await;
    (answer.status, answer.body)
}

fn manifest(kind: &str, name: &str, namespace: &str, spec: Value) -> Value {
    json!({
        "apiVersion": joinedcontext_portal::resource::API_VERSION,
        "kind": kind,
        "metadata": { "name": name, "namespace": namespace },
        "spec": spec,
    })
}

/// The person approved the Change in the Portal and the reconciler read it back.
fn approved(state: &AppState, manifest: &Value) {
    let envelope: joinedcontext_portal::resource::ResourceEnvelope =
        serde_json::from_value(manifest.clone()).expect("an envelope");
    state.mirror.upsert(envelope);
}

/// A Pipeline proposes on a green record check of its spec (T-4840), which `jc_pipeline_test`
/// records; the scenario records it the way that run would.
async fn records_checked(state: &AppState, project: &str, manifest: &Value) {
    use joinedcontext_portal::ops::verdict::{digest_of, RecordsVerdict, Verdict};
    let name = manifest["metadata"]["name"].as_str().expect("a name");
    state
        .drafts
        .put(
            project,
            "Pipeline",
            name,
            manifest.clone(),
            Some(0),
            "steward",
            "person",
        )
        .await
        .expect("the draft");
    let mut verdict = Verdict::green(manifest, None);
    verdict.records = Some(Box::new(RecordsVerdict {
        ok: true,
        checked: 1,
        rejected: 0,
        spec_digest: digest_of(&manifest["spec"]),
        sample_digest: digest_of(&json!([])),
    }));
    state
        .drafts
        .set_verdict(project, "Pipeline", name, verdict)
        .await
        .expect("the verdict");
}

fn proposed(status: StatusCode, body: &Value, what: &str) {
    assert_eq!(status, StatusCode::OK, "{what}: {body}");
    assert_eq!(body["change"]["kind"], "Change", "{what}: {body}");
}

/// Space, Endpoint and Pipeline in `project`, each approved before the next names it.
async fn data_path(who: &Identity, state: &AppState, project: &str, domain: &str) {
    let space = manifest("ContextSpace", "air", project, json!({}));
    let (status, body) = step(
        who,
        state,
        project,
        "jc_resource_propose",
        json!({ "manifest": space }),
    )
    .await;
    proposed(status, &body, "space");
    approved(state, &space);

    let endpoint = manifest(
        "Endpoint",
        "air-public",
        project,
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
    );
    let (status, body) = step(
        who,
        state,
        project,
        "jc_resource_propose",
        json!({ "manifest": endpoint }),
    )
    .await;
    proposed(status, &body, "endpoint");
    // The approved Endpoint carries the slug the platform minted (EP-75).
    let mut minted = endpoint.clone();
    minted["spec"]["slug"] = json!("zt4qm7ge2xdv6ksb3ncf5arw2y");
    approved(state, &minted);

    let pipeline = manifest(
        "Pipeline",
        "air-feed",
        project,
        json!({
            "class": "resident",
            "targetEndpoint": format!("urn:ngsi-ld:Endpoint:{domain}:{project}:air-public"),
            "quotas": { "maxMemoryMb": 128, "cpuMillicores": 250 }
        }),
    );
    records_checked(state, project, &pipeline).await;
    let (status, body) = step(
        who,
        state,
        project,
        "jc_resource_propose",
        json!({ "manifest": pipeline }),
    )
    .await;
    proposed(status, &body, "pipeline");
}

fn organization(name: &str, domain: &str) -> Value {
    manifest(
        "Organization",
        name,
        ORG_NAMESPACE,
        json!({ "domain": domain, "locales": ["en", "fi"], "defaultLocale": "en" }),
    )
}

#[tokio::test]
async fn a_platform_administrator_sets_up_an_organization_a_project_and_its_data_path() {
    let state = scenario_world().await;
    let who = platform();
    let (status, body) = step(
        &who,
        &state,
        ORG_NAMESPACE,
        "jc_resource_propose",
        json!({ "manifest": organization("vnt", "vantaa.fi") }),
    )
    .await;
    proposed(status, &body, "organization");

    let (status, body) = step(
        &who,
        &state,
        ORG_NAMESPACE,
        "jc_project_create",
        json!({ "name": "hel-air", "displayName": "Helsinki air" }),
    )
    .await;
    proposed(status, &body, "project");
    approved(
        &state,
        &manifest(
            "Project",
            "hel-air",
            ORG_NAMESPACE,
            json!({ "organizationRef": "hel" }),
        ),
    );

    data_path(&who, &state, "hel-air", "hel.fi").await;
    let (status, body) = step(
        &who,
        &state,
        "hel-air",
        administration::ORGANIZATION,
        json!({ "aspect": "setup" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn an_organization_administrator_runs_its_own_organization_and_no_other() {
    let state = scenario_world().await;
    let who = organization_admin();
    let (status, body) = step(
        &who,
        &state,
        ORG_NAMESPACE,
        "jc_resource_propose",
        json!({ "manifest": organization("esp", "espoo.fi") }),
    )
    .await;
    proposed(status, &body, "own organization settings");

    for refused in [
        organization("hel", "hel.fi"),
        organization("vnt", "vantaa.fi"),
    ] {
        let (status, body) = step(
            &who,
            &state,
            ORG_NAMESPACE,
            "jc_resource_propose",
            json!({ "manifest": refused }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    // A new project belongs to the installation Organization (ADR-N-058 §2.2).
    let (status, body) = step(
        &who,
        &state,
        ORG_NAMESPACE,
        "jc_project_create",
        json!({ "name": "esp-bus" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    data_path(&who, &state, ESP_PROJECT, "espoo.fi").await;
    let space = manifest("ContextSpace", "bus", HEL_PROJECT, json!({}));
    let (status, body) = step(
        &who,
        &state,
        HEL_PROJECT,
        "jc_resource_propose",
        json!({ "manifest": space }),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "another Organization's project: {status} {body}"
    );
}

#[tokio::test]
async fn a_project_administrator_builds_its_project_and_touches_nothing_else() {
    let state = scenario_world().await;
    let who = project_admin();
    let (status, body) = step(
        &who,
        &state,
        ORG_NAMESPACE,
        "jc_resource_propose",
        json!({ "manifest": organization("esp", "espoo.fi") }),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "organization settings: {status} {body}"
    );
    let (status, body) = step(
        &who,
        &state,
        ESP_PROJECT,
        "jc_project_create",
        json!({ "name": "esp-bus" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    data_path(&who, &state, ESP_PROJECT, "espoo.fi").await;
    let space = manifest("ContextSpace", "bus", HEL_PROJECT, json!({}));
    let (status, body) = step(
        &who,
        &state,
        HEL_PROJECT,
        "jc_resource_propose",
        json!({ "manifest": space }),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "another project: {status} {body}"
    );
}
