//! An App name is one address for the whole organization (AP-14a): the REST door refuses an App
//! another project already declares under that name, the dry run answers the same refusal, and
//! the holder is named only to a caller who may read Apps there (PF-59).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, forge, person, send};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const APPS: &str = "/api/v1/projects/ovzdusie/apps";

/// `wide@hel.fi` proposes and reads Apps across the organization, `narrow@hel.fi` in `ovzdusie`
/// alone. `doprava` already declares the public App `board`.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose", "read"] }] }),
    ));
    for (holder, scope) in [
        ("wide", json!({ "organization": "hel" })),
        ("narrow", json!({ "project": "ovzdusie" })),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{holder}-app-editor"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{holder}@hel.fi") }],
                "role": "app-editor",
                "scope": scope,
            }),
        ));
    }
    state.mirror.upsert(envelope(
        "ContextSpace",
        "ovzdusie",
        "ovzdusie",
        json!({ "isSandbox": false }),
    ));
    state
        .mirror
        .upsert(envelope("App", "board", "doprava", spec("public")));
    state
}

fn spec(visibility: &str) -> Value {
    json!({
        "kind": "static",
        "source": { "path": "./src" },
        "build": { "node": "22" },
        "visibility": visibility,
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "types": ["AirQualityObserved"],
            "operations": ["queryEntity"],
        }],
    })
}

fn app(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": spec("project"),
    })
}

fn detail(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["detail"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned())
}

#[tokio::test]
async fn an_app_name_another_project_declares_is_refused_and_the_holder_named_only_to_a_reader() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let seen = send(&state, person("wide"), "POST", APPS, Some(app("board"))).await;
    assert_eq!(seen.status, StatusCode::FORBIDDEN, "{}", seen.text);
    let said = detail(&seen.text);
    assert!(said.contains("taken by project doprava"), "{said}");
    assert!(said.contains("/apps/board/"), "{said}");

    let blind = send(&state, person("narrow"), "POST", APPS, Some(app("board"))).await;
    assert_eq!(blind.status, StatusCode::FORBIDDEN, "{}", blind.text);
    let said = detail(&blind.text);
    assert!(said.contains("is taken"), "{said}");
    assert!(!said.contains("doprava"), "{said}");
}

#[tokio::test]
async fn the_dry_run_refuses_the_taken_app_name_and_passes_a_free_one() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let checked = format!("{APPS}?dryRun=All");

    let taken = send(
        &state,
        person("narrow"),
        "POST",
        &checked,
        Some(app("board")),
    )
    .await;
    assert_eq!(taken.status, StatusCode::FORBIDDEN, "{}", taken.text);
    assert!(detail(&taken.text).contains("AP-14a"), "{}", taken.text);

    let free = send(
        &state,
        person("narrow"),
        "POST",
        &checked,
        Some(app("board-2")),
    )
    .await;
    assert_eq!(free.status, StatusCode::OK, "{}", free.text);
}

#[tokio::test]
async fn an_import_carrying_an_app_another_project_declares_is_refused_the_same_way() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/git/trees/.*", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tree": [], "truncated": false,
        })))
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    let import = "/api/v1/projects/ovzdusie/import?dryRun=All";

    let taken = send(&state, person("narrow"), "POST", import, Some(app("board"))).await;
    assert_eq!(taken.status, StatusCode::FORBIDDEN, "{}", taken.text);
    let said = detail(&taken.text);
    assert!(
        said.contains("AP-14a") && !said.contains("doprava"),
        "{said}"
    );

    let free = send(
        &state,
        person("narrow"),
        "POST",
        import,
        Some(app("board-2")),
    )
    .await;
    assert_ne!(free.status, StatusCode::FORBIDDEN, "{}", free.text);
}

/// AP-114: a new App whose client `app-{name}` the realm holds and this platform did not make is
/// refused at the REST door and in an import, naming the project and the client; the name stays
/// free for everything else.
#[tokio::test]
async fn a_new_app_whose_client_the_realm_holds_unmanaged_is_refused_at_every_door() {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{}/git/trees/.*", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tree": [], "truncated": false,
        })))
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);
    state
        .foreign_names
        .set_clients(std::collections::BTreeSet::from(["app-radar".to_owned()]));

    for door in [
        format!("{APPS}?dryRun=All"),
        "/api/v1/projects/ovzdusie/import?dryRun=All".to_owned(),
    ] {
        let taken = send(&state, person("narrow"), "POST", &door, Some(app("radar"))).await;
        assert_eq!(
            taken.status,
            StatusCode::FORBIDDEN,
            "{door}: {}",
            taken.text
        );
        let said = detail(&taken.text);
        assert!(
            said.contains("App 'radar' of project ovzdusie")
                && said.contains("'app-radar'")
                && said.contains("AP-114"),
            "{door}: {said}"
        );
    }

    let free = send(
        &state,
        person("narrow"),
        "POST",
        &format!("{APPS}?dryRun=All"),
        Some(app("radar-2")),
    )
    .await;
    assert_eq!(free.status, StatusCode::OK, "{}", free.text);
}

/// AP-124: an App still written `static` passes the dry run, and its verdict says what to write
/// instead; the same App written `ui` passes with no warning.
#[tokio::test]
async fn the_dry_run_warns_an_old_shape_name_and_passes_the_new_one_silently() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let checked = format!("{APPS}?dryRun=All");

    let old = send(
        &state,
        person("narrow"),
        "POST",
        &checked,
        Some(app("air-desk")),
    )
    .await;
    assert_eq!(old.status, StatusCode::OK, "{}", old.text);
    let verdict = &serde_json::from_str::<Value>(&old.text).expect("json")["verdict"];
    assert_eq!(verdict["ok"], json!(true), "{verdict}");
    let warning = verdict["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|f| f["path"] == json!("spec.kind"))
        .unwrap_or_else(|| panic!("a spec.kind warning: {verdict}"));
    assert_eq!(warning["level"], json!("warning"));
    assert!(
        warning["message"]
            .as_str()
            .is_some_and(|m| m.contains("write `kind: ui`")),
        "{warning}"
    );

    let mut new = app("air-desk");
    new["spec"]["kind"] = json!("ui");
    let fresh = send(&state, person("narrow"), "POST", &checked, Some(new)).await;
    assert_eq!(fresh.status, StatusCode::OK, "{}", fresh.text);
    let verdict = &serde_json::from_str::<Value>(&fresh.text).expect("json")["verdict"];
    assert!(
        !verdict["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|f| f["path"] == json!("spec.kind")),
        "{verdict}"
    );
}
