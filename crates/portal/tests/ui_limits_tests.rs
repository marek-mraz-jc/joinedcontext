//! T-1196: the byte ceiling a page enforces is the one the runner enforces.
//!
//! Two limits reach the browser as a number in TypeScript, because a file picker that only
//! learns the limit from a 413 is a worse page than one that says so before the upload: the
//! pipeline test's sample (`jcctl::pipeline_test::MAX_SAMPLE_BYTES`) and the model inference
//! sample (`tools::model_tools::MAX_SAMPLE_BYTES`). Neither is the authority — the routes
//! refuse a larger body whatever the page believes — so the duplicate that matters is a page
//! that refuses what the server would have taken, or takes what the server refuses.
//!
//! This is the whole fix. A `GET /api/v1/limits` route and a promise the page awaits before it
//! can validate a file would be a round trip, a loading state and a fallback constant, to carry
//! a number that changes when someone edits this repository — where this test runs.

use std::fs::read_to_string;

/// The `export const NAME = <expr>;` of a TypeScript module, evaluated for the one shape these
/// two use: a product of integer literals.
fn constant(file: &str, name: &str) -> usize {
    let source = read_to_string(file).unwrap_or_else(|e| panic!("{file}: {e}"));
    let line = source
        .lines()
        .find(|line| line.starts_with(&format!("export const {name} =")))
        .unwrap_or_else(|| panic!("{file} no longer exports {name}"));
    let value = line
        .split_once('=')
        .expect("an assignment")
        .1
        .trim()
        .trim_end_matches(';');
    value
        .split('*')
        .map(|factor| {
            factor
                .trim()
                .parse::<usize>()
                .unwrap_or_else(|e| panic!("{file}: {name} is not a product of literals: {e}"))
        })
        .product()
}

#[test]
fn the_pipeline_test_page_refuses_exactly_what_the_runner_refuses() {
    assert_eq!(
        constant(
            "ui/src/pages/pipelines/PipelineTest.tsx",
            "MAX_SAMPLE_BYTES"
        ),
        jcctl::pipeline_test::MAX_SAMPLE_BYTES,
        "the page and the harness disagree about how large a sample may be"
    );
}

#[test]
fn the_model_file_drop_refuses_exactly_what_model_tools_refuses() {
    assert_eq!(
        constant("ui/src/pages/models/ModelFileDrop.tsx", "MAX_SAMPLE_BYTES"),
        joinedcontext_portal::tools::model_tools::MAX_SAMPLE_BYTES,
        "the drop zone and the inference route disagree about how large a sample may be"
    );
}
