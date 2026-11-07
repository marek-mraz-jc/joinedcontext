//! What a caller may not read is not there, on the removal door too (T-2563, R20, PF-50, PF-59).
//!
//! A person with no read of a kind in a project gets the same 404 for a resource that exists as
//! for one that does not, so names cannot be tested one guess at a time. A person who reads the
//! resource and lacks `delete` still gets the 403 that names the missing grant.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, forge, person, send};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const SPACES: &str = "/api/v1/projects/ovzdusie/spaces";

/// `ovzdusie` holds the space `mobility`. `reader` reads spaces there, `elsewhere` reads spaces in
/// `doprava` only, `remover` reads and deletes them in `ovzdusie`.
fn state_with(gitea: &wiremock::MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "mobility",
        "ovzdusie",
        json!({ "isSandbox": false }),
    ));
    for (role, verbs) in [
        ("space-reader", json!(["read"])),
        ("space-remover", json!(["read", "delete"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": verbs }] }),
        ));
    }
    for (holder, role, project) in [
        ("reader", "space-reader", "ovzdusie"),
        ("elsewhere", "space-reader", "doprava"),
        ("remover", "space-remover", "ovzdusie"),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{holder}-binding"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{holder}@hel.fi") }],
                "role": role,
                "scope": { "project": project },
            }),
        ));
    }
    state
}

fn detail(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["detail"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned())
}

/// R20, PF-59: a caller with no read of the kind in this project cannot tell `mobility`, which
/// exists, from `nothing`, which does not; the words differ only by the name they were given.
#[tokio::test]
async fn a_removal_by_a_caller_who_may_not_read_it_is_not_there() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    for stranger in ["nobody", "elsewhere"] {
        let mut said = Vec::new();
        for name in ["mobility", "nothing"] {
            let answer = send(
                &state,
                person(stranger),
                "DELETE",
                &format!("{SPACES}/{name}?dryRun=All"),
                None,
            )
            .await;
            assert_eq!(
                answer.status,
                StatusCode::NOT_FOUND,
                "{stranger} {name}: {}",
                answer.text
            );
            said.push(detail(&answer.text).replace(name, "…"));
        }
        assert_eq!(said[0], said[1], "{stranger}");
    }
}

/// PF-50: a caller who reads the resource and lacks `delete` is told which grant is missing,
/// and one who holds it gets the removal's dry run.
#[tokio::test]
async fn a_reader_without_delete_gets_the_403_and_a_remover_the_dry_run() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let url = format!("{SPACES}/mobility?dryRun=All");

    let reader = send(&state, person("reader"), "DELETE", &url, None).await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    assert!(detail(&reader.text).contains("delete"), "{}", reader.text);

    let remover = send(&state, person("remover"), "DELETE", &url, None).await;
    assert_eq!(remover.status, StatusCode::OK, "{}", remover.text);

    // A missing name stays a 404 to a caller who could have removed it.
    let missing = send(
        &state,
        person("remover"),
        "DELETE",
        &format!("{SPACES}/nothing?dryRun=All"),
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);
}
