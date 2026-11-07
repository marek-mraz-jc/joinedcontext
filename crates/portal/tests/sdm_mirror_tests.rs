//! The Smart Data Models mirror (T-6353): what it serves, what it refuses to serve, and that the
//! import and the catalogue answer from it with no Model Tools behind them.

mod common;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::tools::sdm_mirror::{Mirrored, SdmMirror};
use tower::ServiceExt;

const PIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const AIR: &str = "dataModel.Environment/AirQualityObserved";

fn linkml(commit: &str) -> String {
    format!("id: https://smartdatamodels.org/air\nannotations:\n  spec.source.commit: {commit}\n")
}

/// A mirror on disk: one model at its pin, one at another commit, one listed without a file, one failed.
fn mirror(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sdm-mirror-{tag}-{stamp}"));
    let folder = |name: &str| root.join("dataModel.Environment").join(name);
    for (name, commit) in [("AirQualityObserved", PIN), ("WaterObserved", OTHER)] {
        std::fs::create_dir_all(folder(name)).unwrap();
        std::fs::write(folder(name).join("linkml.yaml"), linkml(commit)).unwrap();
    }
    let index = serde_json::json!({
        "refreshedAt": "2026-11-02T00:00:00Z",
        "subjects": [{ "name": "dataModel.Environment", "title": "Environment", "models": [
            { "id": AIR, "name": "AirQualityObserved", "attributes": ["pm10"], "commit": PIN },
            { "id": "dataModel.Environment/WaterObserved", "name": "WaterObserved", "commit": PIN },
            { "id": "dataModel.Environment/Gone", "name": "Gone", "commit": PIN },
        ]}],
        "failed": [{ "id": "dataModel.Environment/Broken", "finding": "schema.json answered 404" }],
    });
    std::fs::write(root.join("index.json"), index.to_string()).unwrap();
    root
}

fn config(root: &std::path::Path) -> Config {
    Config {
        sdm_mirror_dir: Some(root.display().to_string()),
        ..Config::for_tests()
    }
}

#[tokio::test]
async fn a_model_at_its_pin_is_served() {
    let open = SdmMirror::open(&mirror("hit")).unwrap();
    assert_eq!(open.model(AIR).await, Mirrored::Model(linkml(PIN)));
}

#[tokio::test]
async fn a_model_the_index_does_not_list_or_whose_file_is_gone_is_a_miss() {
    let open = SdmMirror::open(&mirror("miss")).unwrap();
    assert_eq!(
        open.model("dataModel.Environment/Nothing").await,
        Mirrored::Unknown
    );
    assert_eq!(
        open.model("dataModel.Environment/Gone").await,
        Mirrored::Unknown
    );
}

#[tokio::test]
async fn a_file_at_another_commit_than_the_index_pins_is_not_served() {
    let open = SdmMirror::open(&mirror("stale")).unwrap();
    assert_eq!(
        open.model("dataModel.Environment/WaterObserved").await,
        Mirrored::Unknown
    );
}

#[tokio::test]
async fn a_failed_model_answers_its_finding() {
    let open = SdmMirror::open(&mirror("failed")).unwrap();
    assert_eq!(
        open.model("dataModel.Environment/Broken").await,
        Mirrored::Failed("schema.json answered 404".into())
    );
}

#[tokio::test]
async fn a_traversal_in_an_id_reaches_no_file() {
    let root = mirror("traversal");
    std::fs::write(root.join("secret.yaml"), linkml(PIN)).unwrap();
    let open = SdmMirror::open(&root).unwrap();
    for id in [
        "../secret.yaml",
        "dataModel.Environment/../../secret.yaml",
        "/etc/passwd",
    ] {
        assert_eq!(open.model(id).await, Mirrored::Unknown, "{id}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_leaving_the_mirror_is_not_followed() {
    let root = mirror("link");
    let outside = root.with_extension("outside");
    std::fs::write(&outside, linkml(PIN)).unwrap();
    let file = root.join("dataModel.Environment/AirQualityObserved/linkml.yaml");
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&outside, &file).unwrap();
    let open = SdmMirror::open(&root).unwrap();
    assert_eq!(open.model(AIR).await, Mirrored::Unknown);
}

#[test]
fn an_index_with_an_unusable_id_or_no_commit_does_not_open() {
    let root = mirror("bad-index");
    let bad = r#"{"subjects":[{"name":"s","models":[{"id":"../x/y","name":"y","commit":"a"}]}]}"#;
    std::fs::write(root.join("index.json"), bad).unwrap();
    assert!(SdmMirror::open(&root).is_err());
    assert!(SdmMirror::open(&root.join("nothing")).is_err());
}

#[tokio::test]
async fn the_import_answers_from_the_mirror_with_no_model_tools_behind_it() {
    let state = AppState::new(config(&mirror("route")), None);
    let body = serde_json::json!({ "model": AIR });
    let answer = common::send(
        &state,
        common::person("demo.steward"),
        "POST",
        "/api/v1/tools/import-sdm",
        Some(body),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
    assert_eq!(body["linkml"], linkml(PIN));
}

#[tokio::test]
async fn the_catalogue_lists_the_pins_and_the_failed_with_their_finding() {
    let state = AppState::new(config(&mirror("catalogue")), None);
    let uri = "/api/v1/tools/sdm-catalog?subject=dataModel.Environment";
    let answer = common::send(&state, common::person("demo.steward"), "GET", uri, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
    assert_eq!(body["subjects"][0]["models"][0]["commit"], PIN);
    assert_eq!(body["failed"][0]["finding"], "schema.json answered 404");
    assert_eq!(body["refreshedAt"], "2026-11-02T00:00:00Z");
}

#[tokio::test]
async fn an_anonymous_caller_reads_nothing_of_the_mirror() {
    let state = AppState::new(config(&mirror("anonymous")), None);
    let request = Request::get("/api/v1/tools/sdm-catalog")
        .body(Body::empty())
        .unwrap();
    let response = server::app(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
