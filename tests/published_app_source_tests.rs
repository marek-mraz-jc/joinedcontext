//! A published static App names the repository the build lane builds (AP-87, T-2600).
//!
//! `allerts` and `citiy-bike-station` were published on dev with `source.path: ./src`: nothing
//! could build them, and both answered 404 to everyone they were published for. The door refuses
//! a repeat, accepts the same App from its own repository, accepts the bundle the Portal image
//! ships only where that bundle is, and still lets the old ones be retired.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::MockServer;

use common::{checked_send as send, envelope, forge, person};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const APPS: &str = "/api/v1/projects/helsinki/apps";

/// A Portal whose static host holds `shipped` in `apps_dir`, and `jana`, who may propose Apps.
fn state_with(gitea: &MockServer, apps_dir: &std::path::Path) -> AppState {
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let config = Config {
        apps_dir: Some(apps_dir.to_string_lossy().into_owned()),
        ..Config::for_tests()
    };
    let state = AppState::new(config, None).with_gitea(Arc::new(client));
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App", "ContextSpace"], "verbs": ["propose"] }] }),
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

/// A directory holding the bundle of `shipped`, the way the image lays out `/srv/apps`.
fn apps_dir() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "jc-published-app-source-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(root.join("shipped")).expect("the apps directory");
    std::fs::write(root.join("shipped/index.html"), "<!doctype html>").expect("the bundle");
    root
}

fn app(name: &str, lifecycle: &str, source: Value, shipped_with: Option<&str>) -> Value {
    let mut metadata = json!({ "name": name, "namespace": "helsinki" });
    if let Some(value) = shipped_with {
        metadata["annotations"] = json!({ "joinedcontext.com/shipped-with": value });
    }
    json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": metadata,
        "spec": {
            "kind": "static",
            "source": source,
            "build": { "node": "22" },
            "visibility": "project",
            "lifecycle": lifecycle,
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
                "types": ["BikeHireDockingStation"],
                "operations": ["queryEntity"],
            }],
        },
    })
}

fn folder() -> Value {
    json!({ "path": "./src" })
}

fn repository(name: &str) -> Value {
    json!({ "git": {
        "url": format!("https://git.example/joinedcontext/helsinki_{name}.git"),
        "ref": "0123456789abcdef0123456789abcdef01234567",
    }})
}

/// AP-87: publishing a static App on a folder of the configuration repository is refused with
/// the field and the two ways out; the same App from its own repository is accepted.
#[tokio::test]
async fn a_published_static_app_on_a_folder_is_refused_and_one_on_its_repository_is_accepted() {
    let gitea = forge().await;
    let dir = apps_dir();
    let state = state_with(&gitea, &dir);

    let refused = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app("allerts", "published", folder(), None)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    for words in ["spec.source.git", "retire", "own repository", "AP-87"] {
        assert!(refused.text.contains(words), "{words}: {}", refused.text);
    }

    let accepted = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app("bikes", "published", repository("bikes"), None)),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let _ = std::fs::remove_dir_all(dir);
}

/// AP-87: the bundle the Portal image ships is published without a repository, but only where
/// this Portal holds it; the refusal names the App being written and no other (PF-59).
#[tokio::test]
async fn the_shipped_mark_is_believed_only_where_the_bundle_is() {
    let gitea = forge().await;
    let dir = apps_dir();
    let state = state_with(&gitea, &dir);

    let shipped = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app("shipped", "published", folder(), Some("portal"))),
    )
    .await;
    assert_eq!(shipped.status, StatusCode::ACCEPTED, "{}", shipped.text);

    let claimed = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app("claimed", "published", folder(), Some("portal"))),
    )
    .await;
    assert_eq!(claimed.status, StatusCode::BAD_REQUEST, "{}", claimed.text);
    assert!(
        claimed.text.contains("joinedcontext.com/shipped-with")
            && claimed.text.contains("'claimed'"),
        "{}",
        claimed.text
    );
    assert!(!claimed.text.contains("shipped'"), "{}", claimed.text);
    let _ = std::fs::remove_dir_all(dir);
}

/// AP-87: an App published on a folder before the rule can still be retired, which is
/// the way out the refusal names.
#[tokio::test]
async fn an_app_on_a_folder_can_still_be_retired() {
    let gitea = forge().await;
    let dir = apps_dir();
    let state = state_with(&gitea, &dir);

    let retired = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app("citiy-bike-station", "retired", folder(), None)),
    )
    .await;
    assert_eq!(retired.status, StatusCode::ACCEPTED, "{}", retired.text);
    let _ = std::fs::remove_dir_all(dir);
}
