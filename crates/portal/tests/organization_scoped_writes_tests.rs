//! Writes stay inside the caller's Organization (T-6766, ADR-N-058 section 2.4, TEN-06): over
//! the MCP door, the REST door and the registry's own gate, every writing operation of the
//! registry is refused to a member of `bbsk` in `praha`'s project, so an operation added later is
//! covered without a new line here. A project is opened in the caller's Organization, never in
//! the installation's, which holds operators and no data projects; an Organization's
//! administrator leaves its domain, limits and catalogue to the operators.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use jc_core::kinds::Verb;
use serde_json::{json, Value};

use common::doors::{http, mcp, mcp_rpc};
use common::envelope;
use joinedcontext_portal::api::projects::opening_organization;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops;
use joinedcontext_portal::permissions::{for_request, ORG_NAMESPACE};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const BBSK_PROJECT: &str = "bbsk-air";
const PRAHA_PROJECT: &str = "praha-bus";

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

/// org-admin of `bbsk`.
fn bbsk_admin() -> Identity {
    someone("bbadmin")
}

/// steward of `bbsk-air`.
fn bbsk_steward() -> Identity {
    someone("bbsteward")
}

/// org-admin of the installation Organization `ops`.
fn operator() -> Identity {
    someone("operator")
}

/// org-admin of both `bbsk` and `praha`.
fn twice_admin() -> Identity {
    someone("twice")
}

fn binding(name: &str, user: &str, role: &str, scope: Value) -> Value {
    json!({ "name": name, "spec": { "subjects": [{ "user": format!("{user}@example.org") }], "role": role, "scope": scope } })
}

fn bbsk_organization() -> Value {
    json!({ "domain": "bbsk.sk", "locales": ["sk"], "defaultLocale": "sk", "limits": { "uploadMiB": 10 } })
}

/// Three Organizations: the installation's `ops` of operators alone, and `bbsk` and `praha`,
/// each with one project.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    for (name, spec) in [
        (
            "ops",
            json!({ "domain": "ops.example", "installation": true }),
        ),
        ("bbsk", bbsk_organization()),
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
        m.upsert(envelope("ContextSpace", "air", project, json!({})));
    }
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    let every =
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }] });
    m.upsert(envelope("Role", "org-admin", ORG_NAMESPACE, every.clone()));
    m.upsert(envelope("Role", "steward", ORG_NAMESPACE, every));
    for b in [
        binding(
            "operators",
            "operator",
            "org-admin",
            json!({ "organization": "ops" }),
        ),
        binding(
            "bbsk-admins",
            "bbadmin",
            "org-admin",
            json!({ "organization": "bbsk" }),
        ),
        binding(
            "bbsk-air-steward",
            "bbsteward",
            "steward",
            json!({ "project": BBSK_PROJECT }),
        ),
        binding(
            "twice-bbsk",
            "twice",
            "org-admin",
            json!({ "organization": "bbsk" }),
        ),
        binding(
            "twice-praha",
            "twice",
            "org-admin",
            json!({ "organization": "praha" }),
        ),
    ] {
        m.upsert(envelope(
            "RoleBinding",
            b["name"].as_str().unwrap_or_default(),
            ORG_NAMESPACE,
            b["spec"].clone(),
        ));
    }
    state
}

/// The registry's writing operations inside a project; the ones decided at the Organization
/// (people, opening a project) are judged by the tests below them.
fn writes() -> Vec<&'static ops::Operation> {
    ops::registry()
        .iter()
        .filter(|op| !op.annotations.read_only_hint)
        .filter(|op| ops::requires(op).checked_by != "organization")
        .collect()
}

/// A call of `op` aimed at `praha`'s project that its schema and parser accept, so the door's
/// answer is its decision on the caller, not on the shape of the input: the schema's example, or
/// a real target where the example names none.
fn input_for(op: &ops::Operation) -> Value {
    let space = json!({ "apiVersion": joinedcontext_portal::resource::API_VERSION, "kind": "ContextSpace",
        "metadata": { "name": "bus", "namespace": PRAHA_PROJECT }, "spec": {} });
    match op.name {
        "jc_resource_propose" => json!({ "manifest": space }),
        "jc_resource_delete" => json!({ "kind": "ContextSpace", "name": "air", "confirm": "air" }),
        "jc_project_delete" => json!({ "name": PRAHA_PROJECT }),
        "jc_project_import" => {
            json!({ "bundle": serde_json::to_string(&space).unwrap_or_default(), "conflictPolicy": "fail" })
        }
        _ => ops::example_of(&(op.input)()),
    }
}

fn refused(status: StatusCode) -> bool {
    status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND
}

/// Point 1: the table. Each registry write in `praha`'s project, by `bbsk`'s administrator and
/// by `bbsk`'s steward, is refused by the gate every door asks first, by the MCP door before the
/// person is asked to confirm, and by the REST door.
#[tokio::test]
async fn every_registry_write_is_refused_in_another_organizations_project() {
    let state = world();
    let operations = writes();
    assert!(operations.len() > 20, "the registry lists its writes");
    let mut leaks = Vec::new();
    for who in [bbsk_admin(), bbsk_steward()] {
        for op in &operations {
            let input = input_for(op);
            if let Err(err) = ops::may_try(op, &who, &state, PRAHA_PROJECT, Some(&input)) {
                let status = err.into_response().status();
                if !refused(status) {
                    leaks.push(format!("gate {} {}: {status}", who.username, op.name));
                }
            } else {
                leaks.push(format!("gate {} {}: allowed", who.username, op.name));
            }

            let mut arguments = input.clone();
            if let Some(members) = arguments.as_object_mut() {
                members.insert("project".to_owned(), json!(PRAHA_PROJECT));
            }
            let answer = mcp_rpc(
                &mcp(who.clone()),
                &state,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                    "name": op.name, "arguments": arguments } }),
            )
            .await;
            if answer["result"]["isError"] != json!(true)
                || answer["result"]["status"] == json!("input_required")
            {
                leaks.push(format!("mcp {} {}: {answer}", who.username, op.name));
            }

            let rest = http(
                &state,
                who.clone(),
                "POST",
                &format!("/api/v1/projects/{PRAHA_PROJECT}/ops/{}", op.name),
                Some(input.clone()),
            )
            .await;
            if !refused(rest.status) {
                leaks.push(format!(
                    "rest {} {}: {} {}",
                    who.username, op.name, rest.status, rest.body
                ));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// Point 6: a Change of `praha` is approved only by a person holding approve in `praha`.
#[test]
fn a_change_is_approved_only_by_a_holder_of_the_right_in_its_organization() {
    let state = world();
    let space = json!({ "kind": "ContextSpace", "metadata": { "name": "bus", "namespace": PRAHA_PROJECT }, "spec": {} });
    for who in [bbsk_admin(), bbsk_steward(), operator()] {
        assert!(
            for_request(&state, &who, PRAHA_PROJECT)
                .check("ContextSpace", Verb::Approve, Some(&space))
                .is_err(),
            "{} approves in praha",
            who.username
        );
    }
    assert!(for_request(&state, &twice_admin(), PRAHA_PROJECT)
        .check("ContextSpace", Verb::Approve, Some(&space))
        .is_ok());
}

/// Point 2 and 4: a project opens in the caller's Organization, a named one only where the
/// caller holds the right, and never in the installation's.
#[test]
fn a_project_opens_in_the_callers_organization_and_never_in_the_installations() {
    let state = world();
    let status = |result: Result<String, joinedcontext_portal::error::ApiError>| {
        result.map_err(|err| err.into_response().status())
    };
    assert_eq!(
        status(opening_organization(&state, &bbsk_admin(), None)),
        Ok("bbsk".to_owned())
    );
    assert_eq!(
        status(opening_organization(&state, &bbsk_admin(), Some("praha"))),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        status(opening_organization(&state, &bbsk_admin(), Some("nowhere"))),
        Err(StatusCode::NOT_FOUND)
    );
    // A project's steward holds no right at the Organization.
    assert_eq!(
        status(opening_organization(&state, &bbsk_steward(), None)),
        Err(StatusCode::FORBIDDEN)
    );
    // The installation's Organization holds operators and no data projects.
    assert_eq!(
        status(opening_organization(&state, &operator(), None)),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        status(opening_organization(&state, &operator(), Some("ops"))),
        Err(StatusCode::FORBIDDEN)
    );
    // Holding the right in two, the caller names one.
    assert_eq!(
        status(opening_organization(&state, &twice_admin(), None)),
        Err(StatusCode::UNPROCESSABLE_ENTITY)
    );
    assert_eq!(
        status(opening_organization(&state, &twice_admin(), Some("praha"))),
        Ok("praha".to_owned())
    );
}

/// Point 3: `bbsk`'s administrator edits its own Organization's settings but not its domain,
/// limits or catalogue, and creates no Organization.
#[test]
fn an_organization_administrator_leaves_domain_limits_and_catalogue_to_the_operators() {
    let state = world();
    let at_org = for_request(&state, &bbsk_admin(), ORG_NAMESPACE);
    let manifest = |spec: Value| json!({ "kind": "Organization", "metadata": { "name": "bbsk", "namespace": ORG_NAMESPACE }, "spec": spec });
    let mut locales = bbsk_organization();
    locales["locales"] = json!(["sk", "en"]);
    assert!(at_org
        .check("Organization", Verb::Propose, Some(&manifest(locales)))
        .is_ok());
    for (pointer, value) in [
        ("/limits", json!({ "uploadMiB": 1000 })),
        ("/domain", json!("praha.eu")),
        ("/catalog", json!({ "publisher": "someone" })),
        ("/gitRepositoryUrl", json!("https://forge.example/x.git")),
    ] {
        let mut spec = bbsk_organization();
        spec[pointer.trim_start_matches('/')] = value;
        let refusal = at_org
            .check("Organization", Verb::Propose, Some(&manifest(spec)))
            .expect_err(pointer);
        assert!(refusal.to_string().contains("operators"), "{refusal}");
    }
    let new = json!({ "kind": "Organization", "metadata": { "name": "kosice", "namespace": ORG_NAMESPACE }, "spec": { "domain": "kosice.sk" } });
    assert!(at_org
        .check("Organization", Verb::Propose, Some(&new))
        .is_err());
    // The operators set them.
    assert!(for_request(&state, &operator(), ORG_NAMESPACE)
        .check("Organization", Verb::Propose, Some(&new))
        .is_ok());
}
