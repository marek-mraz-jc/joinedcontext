//! The project operations through every door of the registry (T-1528; AG-64, PF-50, PF-59, PF-77,
//! AG-77, MF-18).
//!
//! What was there before this file: the REST routes are tested in `projects_api_tests.rs` (the
//! read, and the deletion as an administrator, a steward and a stranger), `export_api_tests.rs`
//! (the export narrowed to a space grant, the revision picker, 404 without a binding) and
//! `edge_readonly_doors_tests.rs`. `attack_person_only_operations_tests.rs` names
//! `jc_project_delete` only to prove a run may propose it (AG-77). No test called
//! `jc_project_get`, `jc_project_revisions` or `jc_project_export` by name, and none compared
//! a door's refusal with the route's.

mod common;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::{encode, envelope, forge, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8";

const FILES: &[(&str, &str)] = &[
    (
        "projects/ovzdusie/spaces/air/space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: air\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n",
    ),
    (
        "projects/ovzdusie/spaces/noise/space.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: noise\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n",
    ),
    (
        "projects/ovzdusie/endpoints/air-public.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air-public\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: air\n  slug: mluyob4nz52lok3ssk7pgn5vwt\n  audience: public\n",
    ),
    (
        "projects/ovzdusie/endpoints/noise-public.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: noise-public\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: noise\n  slug: zt4qm7ge2xdv6ksb3ncf5arw2y\n  audience: public\n",
    ),
    (
        "projects/doprava/endpoints/buses.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: buses\n  namespace: doprava\nspec:\n  slug: k7m2qz4tv6xh3n5jb2ryd3wcfa\n",
    ),
];

/// Reads the air space of [`PROJECT`] and nothing else of it.
fn zuzana() -> Identity {
    Identity {
        client: None,
        subject: "f:1:zuzana".into(),
        username: "zuzana".into(),
        email: Some("zuzana@banskabystrica.sk".into()),
        name: Some("zuzana".into()),
        roles: Vec::new(),
        groups: vec!["air-readers".into()],
        second_factor: false,
    }
}

async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/main")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": REVISION } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/git/trees/.*")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "truncated": false,
            "tree": FILES.iter().map(|(file, _)| json!({ "path": file, "type": "blob" })).collect::<Vec<_>>(),
        })))
        .mount(&server)
        .await;
    for (file, content) in FILES {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/{file}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "sha": "blob-1", "content": encode(content) })),
            )
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/commits")))
        .and(query_param("path", "projects/ovzdusie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "sha": REVISION,
            "commit": {
                "message": "Endpoint air-public: add csv",
                "author": { "name": "Jana", "date": "2026-09-06T16:30:00Z" }
            }
        }])))
        .mount(&server)
        .await;

    let state = doors::state_with(state_on(&server));
    state.mirror.upsert(envelope(
        "Project",
        PROJECT,
        ORG_NAMESPACE,
        json!({ "organizationRef": { "name": "bb" } }),
    ));
    for space in ["air", "noise"] {
        state.mirror.upsert(envelope(
            "ContextSpace",
            space,
            PROJECT,
            json!({ "isSandbox": false }),
        ));
    }
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers-binding",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "air-readers" }],
            "role": "viewer-role",
            "scope": { "contextSpace": "air" },
        }),
    ));
    (server, state)
}

fn delete_input() -> Value {
    json!({ "name": PROJECT })
}

/// The paths the deletion removed on its branch.
async fn removed(server: &MockServer) -> Vec<String> {
    common::removed_in_one_commit(server).await
}

/// PF-50, AG-64: the steward and the viewer read the project, its history and the manifests they
/// name through the session, MCP and a run whose profile names the three reads.
#[tokio::test]
async fn every_door_reads_the_project_its_history_and_its_export() {
    let (_server, state) = world().await;
    let reads = [
        "jc_project_get",
        "jc_project_revisions",
        "jc_project_export",
    ];
    for (who, caller) in [
        ("steward", session(steward())),
        ("viewer", session(viewer())),
        ("viewer over mcp", mcp(viewer())),
        ("viewer's run", run(viewer(), &reads)),
    ] {
        let project = doors::call("jc_project_get", &caller, &state, json!({})).await;
        assert_eq!(StatusCode::OK, project.status, "{who}: {}", project.text());
        assert_eq!(PROJECT, project.body["metadata"]["name"], "{who}");

        let history = doors::call("jc_project_revisions", &caller, &state, json!({})).await;
        assert_eq!(StatusCode::OK, history.status, "{who}: {}", history.text());
        assert_eq!(REVISION, history.body["items"][0]["sha"], "{who}");

        let export = doors::call(
            "jc_project_export",
            &caller,
            &state,
            json!({ "names": "air-public,noise-public" }),
        )
        .await;
        assert_eq!(StatusCode::OK, export.status, "{who}: {}", export.text());
        let document = export.body["document"].as_str().unwrap_or_default();
        for name in ["air-public", "noise-public"] {
            assert!(
                document.contains(name),
                "{who}: {name} missing from {document}"
            );
        }
        assert!(
            !document.contains("buses"),
            "{who}: another project exported"
        );
    }
}

/// MF-18, PF-59: a person bound to one space exports that space and nothing of the other, at every
/// door.
#[tokio::test]
async fn a_reader_of_one_space_exports_only_that_space() {
    let (_server, state) = world().await;
    for caller in [
        session(zuzana()),
        mcp(zuzana()),
        run(zuzana(), &["jc_project_export"]),
    ] {
        let export = doors::call(
            "jc_project_export",
            &caller,
            &state,
            json!({ "names": "air-public,noise-public" }),
        )
        .await;
        assert_eq!(StatusCode::OK, export.status, "{}", export.text());
        let document = export.body["document"].as_str().unwrap_or_default();
        assert!(document.contains("air-public"), "{document}");
        assert!(
            !document.contains("noise"),
            "the unbound space leaked: {document}"
        );
    }
}

/// UI-87 (T-2879): the whole project leaves only through an organization administrator, at every
/// door; a steward of the project, who approves everything in it, is refused with the reason.
#[tokio::test]
async fn a_whole_project_export_is_for_an_organization_administrator_ui87() {
    let (_server, state) = world().await;
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "org-admins",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "group": "org-admins" }], "role": "org-admin", "scope": { "organization": "bb" } }),
    ));
    let mut admin = steward();
    admin.groups.push("org-admins".into());

    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_project_export"]),
    ] {
        let refused = doors::call("jc_project_export", &caller, &state, json!({})).await;
        assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
        assert!(
            refused.text().contains("organization administrators"),
            "the refusal says who may: {}",
            refused.text()
        );
    }
    for caller in [session(admin.clone()), mcp(admin)] {
        let export = doors::call("jc_project_export", &caller, &state, json!({})).await;
        assert_eq!(StatusCode::OK, export.status, "{}", export.text());
        let document = export.body["document"].as_str().unwrap_or_default();
        for name in ["air-public", "noise-public"] {
            assert!(document.contains(name), "{name} missing from {document}");
        }
    }
}

/// PF-59, R20: a person with no binding in the project gets the same 404 from all four operations
/// at every door, and nothing of the project in it.
#[tokio::test]
async fn a_stranger_finds_no_project_at_any_door() {
    let (server, state) = world().await;
    for caller in [
        session(stranger()),
        mcp(stranger()),
        run(
            stranger(),
            &[
                "jc_project_get",
                "jc_project_revisions",
                "jc_project_export",
                "jc_project_delete",
            ],
        ),
    ] {
        for (name, input) in [
            ("jc_project_get", json!({})),
            ("jc_project_revisions", json!({})),
            ("jc_project_export", json!({})),
            ("jc_project_delete", delete_input()),
        ] {
            let refused = doors::call(name, &caller, &state, input).await;
            assert_eq!(
                StatusCode::NOT_FOUND,
                refused.status,
                "{name}: {}",
                refused.text()
            );
            assert!(
                !refused.text().contains("air"),
                "{name}: {}",
                refused.text()
            );
        }
    }
    assert_eq!(Vec::<String>::new(), removed(&server).await);
}

/// PF-77, PF-50: a deletion is refused to whoever holds no `delete` on the Project (a viewer, and a
/// viewer's run however wide its profile), before one file is touched.
#[tokio::test]
async fn a_deletion_is_refused_to_everyone_without_delete_on_the_project() {
    let (server, state) = world().await;
    for (who, caller) in [
        ("viewer", session(viewer())),
        ("viewer over mcp", mcp(viewer())),
        ("viewer's run", run(viewer(), &["jc_project_delete"])),
        ("reader of one space", session(zuzana())),
        (
            "steward's run without it",
            run(steward(), &["jc_project_get"]),
        ),
    ] {
        let refused = doors::call("jc_project_delete", &caller, &state, delete_input()).await;
        assert_eq!(
            StatusCode::FORBIDDEN,
            refused.status,
            "{who}: {}",
            refused.text()
        );
    }
    assert_eq!(
        Vec::<String>::new(),
        removed(&server).await,
        "a refused deletion touched the tree"
    );
}

/// PF-77: the steward's deletion is one Red Change that names each file it removes, and the other
/// project's files stay.
#[tokio::test]
async fn a_deletion_is_one_red_change_naming_what_it_removes() {
    let (server, state) = world().await;
    let change = doors::call(
        "jc_project_delete",
        &session(steward()),
        &state,
        delete_input(),
    )
    .await;
    assert_eq!(StatusCode::OK, change.status, "{}", change.text());
    assert_eq!(
        "red",
        change.body["change"]["status"]["lane"],
        "{}",
        change.text()
    );
    assert_eq!(
        json!(4),
        change.body["change"]["status"]["plan"]["delete"],
        "{}",
        change.text()
    );
    assert_eq!(
        vec![
            "projects/ovzdusie/endpoints/air-public.yaml",
            "projects/ovzdusie/endpoints/noise-public.yaml",
            "projects/ovzdusie/spaces/air/space.yaml",
            "projects/ovzdusie/spaces/noise/space.yaml",
        ],
        removed(&server).await,
    );
}

/// AG-77, AG-11: a run may propose the deletion only when its profile names it, and what it gets
/// is the same Red Change a person approves; it is offered the operation only then.
#[tokio::test]
async fn a_run_is_offered_the_deletion_only_when_its_profile_names_it() {
    let (_server, state) = world().await;
    let reader = run(steward(), &["jc_project_get", "jc_project_export"]);
    assert!(!doors::offered(&reader, &state).contains(&"jc_project_delete".to_owned()));

    let proposer = run(steward(), &["jc_project_delete"]);
    assert!(doors::offered(&proposer, &state).contains(&"jc_project_delete".to_owned()));
    let change = doors::call("jc_project_delete", &proposer, &state, delete_input()).await;
    assert_eq!(StatusCode::OK, change.status, "{}", change.text());
    assert_eq!(
        "red",
        change.body["change"]["status"]["lane"],
        "{}",
        change.text()
    );
}

/// Input is validated at the door: an export in a format an operation cannot answer, a history
/// longer than the route serves and an unknown field are refused.
#[tokio::test]
async fn an_input_the_route_would_refuse_is_refused_at_the_door() {
    let (_server, state) = world().await;
    let caller = session(steward());
    for (name, input) in [
        ("jc_project_export", json!({ "format": "zip" })),
        ("jc_project_revisions", json!({ "limit": 0 })),
        ("jc_project_get", json!({ "verbose": true })),
        (
            "jc_project_delete",
            json!({ "name": PROJECT, "force": true }),
        ),
    ] {
        let refused = doors::call(name, &caller, &state, input).await;
        assert!(
            refused.status.is_client_error() && refused.status != StatusCode::NOT_FOUND,
            "{name}: {} {}",
            refused.status,
            refused.text()
        );
    }
}

/// AG-64: the REST routes and `POST …/ops/{name}` answer each person alike.
#[tokio::test]
async fn the_rest_routes_and_the_ops_door_answer_alike() {
    let (_server, state) = world().await;
    for identity in [viewer(), stranger(), zuzana()] {
        for (name, rest_method, uri, input) in [
            (
                "jc_project_get",
                "GET",
                "/api/v1/projects/ovzdusie",
                json!({}),
            ),
            (
                "jc_project_revisions",
                "GET",
                "/api/v1/projects/ovzdusie/revisions",
                json!({}),
            ),
            (
                "jc_project_export",
                "GET",
                "/api/v1/projects/ovzdusie/export?format=json",
                json!({ "format": "json" }),
            ),
            (
                "jc_project_delete",
                "DELETE",
                "/api/v1/projects/ovzdusie",
                delete_input(),
            ),
        ] {
            let rest = doors::http(&state, identity.clone(), rest_method, uri, None).await;
            let door = doors::post_op(&state, identity.clone(), name, input).await;
            assert_eq!(
                rest.status,
                door.status,
                "{} {name}: {} vs {}",
                identity.username,
                rest.text(),
                door.text()
            );
        }
    }
}

/// JC1-01: the ceiling of a run (and of any caller) is judged on the project the call is addressed
/// to, so the deletion names that project and no other. A steward bound in both projects whose run
/// works in `ovzdusie` does not propose the removal of `doprava` by naming it.
#[tokio::test]
async fn a_deletion_names_the_project_the_call_is_addressed_to() {
    let (server, state) = world().await;
    state.mirror.upsert(envelope(
        "Project",
        "doprava",
        ORG_NAMESPACE,
        json!({ "organizationRef": { "name": "bb" } }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "steward-binding-doprava",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "city-stewards" }],
            "role": "steward-role",
            "scope": { "project": "doprava" },
        }),
    ));
    for (who, caller) in [
        ("steward", session(steward())),
        ("steward over mcp", mcp(steward())),
        ("steward's run", run(steward(), &["jc_project_delete"])),
    ] {
        let refused = doors::call(
            "jc_project_delete",
            &caller,
            &state,
            json!({ "name": "doprava" }),
        )
        .await;
        assert_eq!(
            StatusCode::UNPROCESSABLE_ENTITY,
            refused.status,
            "{who}: {}",
            refused.text()
        );
        assert_eq!(
            "name",
            refused.body["errors"][0]["path"],
            "{who}: {}",
            refused.text()
        );
    }
    assert_eq!(
        Vec::<String>::new(),
        removed(&server).await,
        "a deletion of another project touched the tree"
    );
}

/// JC1-02, AG-70: an agent run reads over MCP `resources/*` only the kinds its profile grants
/// `read`, as its tools do; the person's own MCP client reads what the person reads.
#[tokio::test]
async fn a_run_reads_no_resource_of_a_kind_its_profile_does_not_grant() {
    let (_server, state) = world().await;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list", "params": { "project": PROJECT } });
    let uris = |answer: &Value| -> Vec<String> {
        answer["result"]["resources"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter_map(|r| r["uri"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let person = uris(&doors::mcp_rpc(&mcp(steward()), &state, list.clone()).await);
    let space = person
        .iter()
        .find(|uri| uri.starts_with(&format!("jc://{PROJECT}/")) && uri.ends_with("/air"))
        .unwrap_or_else(|| panic!("the steward's own client lists the air space: {person:?}"))
        .clone();

    let narrow = run(steward(), &["jc_project_get"]);
    let listed = uris(&doors::mcp_rpc(&narrow, &state, list).await);
    assert!(!listed.contains(&space), "{listed:?}");
    let read = doors::mcp_rpc(
        &narrow,
        &state,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/read", "params": { "uri": space } }),
    )
    .await;
    assert_eq!(-32002, read["error"]["code"], "{read}");
}
