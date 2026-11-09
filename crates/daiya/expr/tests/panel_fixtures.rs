#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // The test reads JC_WRITE_SERVED to rewrite its fixtures; jc runs none of this.
//! The Portal's dashboard e2e replays a panel's frames as daiya answers them: after the panel's
//! transformations. Each fixture of a transformed panel
//! (`crates/portal/ui/tests/fixtures/frames/<dashboard>-<panel>.json`) carries that answer as
//! `served`, and this test holds it equal to `apply_transformations` over its `frames`.
//!
//!   JC_WRITE_SERVED=1 cargo test -p daiya-expr --test panel_fixtures   rewrites `served`

use daiya_expr::tx::apply_transformations;
use daiya_model::{DataTransformerConfig, Frame};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../portal/ui/tests/fixtures")
}

fn read(path: &PathBuf) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("fixture reads"))
        .expect("fixture is JSON")
}

#[test]
fn every_transformed_panel_fixture_serves_what_daiya_answers() {
    let write = std::env::var_os("JC_WRITE_SERVED").is_some();
    let mut checked = 0;
    let mut stale = Vec::new();
    let mut boards: Vec<_> = fs::read_dir(fixtures().join("dashboards"))
        .expect("dashboards dir")
        .map(|entry| entry.expect("entry").path())
        .collect();
    boards.sort();
    for board in boards {
        let stem = board
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("stem")
            .to_owned();
        let dashboard = read(&board);
        for panel in dashboard["panels"].as_array().into_iter().flatten() {
            let Some(steps) = panel["transformations"]
                .as_array()
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let path = fixtures()
                .join("frames")
                .join(format!("{stem}-{}.json", panel["id"]));
            if !path.exists() {
                continue;
            }
            let steps: Vec<DataTransformerConfig> =
                serde_json::from_value(Value::Array(steps.clone())).expect("transformations parse");
            let mut fixture = read(&path);
            let frames: Vec<Frame> =
                serde_json::from_value(fixture["frames"].clone()).expect("frames parse");
            let outcome = apply_transformations(frames, &steps);
            assert!(
                outcome.warnings.is_empty(),
                "{stem}-{}: {:?}",
                panel["id"],
                outcome.warnings
            );
            let served = serde_json::to_value(&outcome.frames).expect("frames serialize");
            checked += 1;
            if fixture["served"] == served {
                continue;
            }
            if write {
                fixture["served"] = served;
                fs::write(
                    &path,
                    format!(
                        "{}\n",
                        serde_json::to_string_pretty(&fixture).expect("json")
                    ),
                )
                .expect("fixture writes");
            } else {
                stale.push(format!("{stem}-{}", panel["id"]));
            }
        }
    }
    assert!(
        checked >= 4,
        "the bbsk and helsinki tables are transformed: {checked} checked"
    );
    assert!(
        stale.is_empty(),
        "stale `served`, rerun with JC_WRITE_SERVED=1: {stale:?}"
    );
}
