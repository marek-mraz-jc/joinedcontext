//! The `{space}` segment a Context Space renders is unique in the organization (PF-44, PF-76,
//! PF-84): the REST door refuses a space whose segment another space pins, and says who holds
//! it only to a caller who may read that project (PF-59). The dry run answers the same refusal
//! as the write.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, forge, person, send};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const SPACES: &str = "/api/v1/projects/doprava/spaces";

/// `wide@hel.fi` proposes and reads spaces across the organization; `narrow@hel.fi` does the
/// same in `doprava` alone. `helsinki` already holds a space pinned to `doprava-mhd`, the
/// segment `doprava`'s `mhd` would render.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "Role",
        "space-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["propose", "read"] }] }),
    ));
    for (holder, scope) in [
        ("wide", json!({ "organization": "hel" })),
        ("narrow", json!({ "project": "doprava" })),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{holder}-space-editor"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{holder}@hel.fi") }],
                "role": "space-editor",
                "scope": scope,
            }),
        ));
    }
    state.mirror.upsert(envelope(
        "ContextSpace",
        "mhd",
        "helsinki",
        json!({ "isSandbox": false, "urnSegment": "doprava-mhd" }),
    ));
    state
}

fn space(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": name, "namespace": "doprava" },
        "spec": { "isSandbox": false },
    })
}

fn detail(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["detail"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned())
}

#[tokio::test]
async fn a_name_another_project_holds_is_refused_and_the_holder_is_named_only_to_a_reader() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let seen = send(&state, person("wide"), "POST", SPACES, Some(space("mhd"))).await;
    assert_eq!(seen.status, StatusCode::FORBIDDEN, "{}", seen.text);
    let said = detail(&seen.text);
    assert!(said.contains("taken by project helsinki"), "{said}");
    assert!(said.contains("doprava-mhd"), "{said}");

    // The same collision, to someone bound nowhere near helsinki: taken, and by nobody named.
    let blind = send(&state, person("narrow"), "POST", SPACES, Some(space("mhd"))).await;
    assert_eq!(blind.status, StatusCode::FORBIDDEN, "{}", blind.text);
    let said = detail(&blind.text);
    assert!(said.contains("is taken"), "{said}");
    assert!(!said.contains("helsinki"), "{said}");
    // The segment is named: it is what the caller's own name renders, no secret of another project.
    assert!(said.contains("doprava-mhd"), "{said}");
}

#[tokio::test]
async fn the_dry_run_refuses_the_taken_name_and_passes_a_free_one() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let checked = send(
        &state,
        person("narrow"),
        "POST",
        &format!("{SPACES}?dryRun=All"),
        Some(space("mhd")),
    )
    .await;
    assert_eq!(checked.status, StatusCode::FORBIDDEN, "{}", checked.text);
    assert!(
        detail(&checked.text).contains("is taken"),
        "{}",
        checked.text
    );

    // A name no project holds is proposed as it was typed.
    let free = send(
        &state,
        person("narrow"),
        "POST",
        &format!("{SPACES}?dryRun=All"),
        Some(space("parkovanie")),
    )
    .await;
    assert_eq!(free.status, StatusCode::OK, "{}", free.text);
}

#[tokio::test]
async fn a_project_may_still_update_its_own_space_of_that_name() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "vlaky",
        "doprava",
        json!({ "isSandbox": false }),
    ));

    let own = send(
        &state,
        person("narrow"),
        "POST",
        &format!("{SPACES}?dryRun=All"),
        Some(space("vlaky")),
    )
    .await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text);
}

/// PF-84: an import is refused a space whose segment another project pins, in the same words as
/// the single-manifest door (T-2561).
#[tokio::test]
async fn an_import_carrying_a_taken_segment_is_refused_the_same_way() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/git/trees/.*", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tree": [], "truncated": false,
        })))
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    let import = "/api/v1/projects/doprava/import?dryRun=All";

    let taken = send(&state, person("narrow"), "POST", import, Some(space("mhd"))).await;
    assert_eq!(taken.status, StatusCode::FORBIDDEN, "{}", taken.text);
    let said = detail(&taken.text);
    assert!(
        said.contains("is taken") && !said.contains("helsinki"),
        "{said}"
    );

    let free = send(
        &state,
        person("narrow"),
        "POST",
        import,
        Some(space("parkovanie")),
    )
    .await;
    assert_ne!(free.status, StatusCode::FORBIDDEN, "{}", free.text);
}
