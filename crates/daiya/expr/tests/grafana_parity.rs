#![forbid(unsafe_code)]
//! daiya's `__expr__` pipeline against Grafana 12.4.14's own answers
//! (`tests/fixtures/grafana-expressions.json`, recorded by `tests/fixtures/record-grafana.py`):
//! Grafana's A frames go in, and every math, reduce, resample and threshold node must come out
//! with Grafana's frames, labels and values (NaN, null and the infinities told apart).

use daiya_expr::{ExprNodeSpec, evaluate_pipeline};
use daiya_model::{Frame, FrameSet};
use serde_json::Value;
use std::collections::BTreeSet;

const RECORDED: &str = include_str!("fixtures/grafana-expressions.json");

/// A value as a comparable word: `null`, `NaN`, `+Inf`, `-Inf` or the number.
fn word(frame: &Frame, field: usize, row: usize) -> String {
    let column = &frame.fields[field];
    if column.values.get(row).is_none_or(Value::is_null) {
        return "null".into();
    }
    match column.f64_at(row) {
        Some(v) if v.is_nan() => "NaN".into(),
        Some(v) if v == f64::INFINITY => "+Inf".into(),
        Some(v) if v == f64::NEG_INFINITY => "-Inf".into(),
        Some(v) => format!("{:.12}", v),
        None => format!("{}", column.values[row]),
    }
}

/// Each frame as `labels: values` lines, so a difference reads as text.
fn lines(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .map(|frame| {
            let mut out = Vec::new();
            for (at, field) in frame.fields.iter().enumerate() {
                let values: Vec<String> =
                    (0..field.len()).map(|row| word(frame, at, row)).collect();
                out.push(format!(
                    "{:?} {:?}: {}",
                    field.field_type,
                    field.labels,
                    values.join(" ")
                ));
            }
            out.join(" | ")
        })
        .collect()
}

#[test]
fn every_expression_answers_what_grafana_answers() {
    let recorded: Value = serde_json::from_str(RECORDED).expect("the recording");
    let from = recorded["from"].as_i64().expect("from");
    let to = recorded["to"].as_i64().expect("to");
    let mut differences = Vec::new();
    let scenarios = recorded["scenarios"].as_object().expect("scenarios");
    for (name, scenario) in scenarios {
        let input: Vec<Frame> =
            serde_json::from_value(scenario["results"]["A"]["frames"].clone()).expect("A frames");
        let nodes: Vec<ExprNodeSpec> = scenario["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .map(|node| ExprNodeSpec::from_json(node, 100).expect("a node daiya reads"))
            .collect();
        let answered = evaluate_pipeline(
            &[FrameSet::new("A", input)],
            &BTreeSet::new(),
            &nodes,
            from,
            to,
            100,
        );
        let sets = match answered {
            Ok(output) => output.frame_sets,
            Err(err) => {
                differences.push(format!("{name}: daiya refused: {err}"));
                continue;
            }
        };
        for node in scenario["nodes"].as_array().expect("nodes") {
            let ref_id = node["refId"].as_str().expect("refId");
            let want: Vec<Frame> =
                serde_json::from_value(scenario["results"][ref_id]["frames"].clone())
                    .expect("Grafana's frames");
            let got = sets
                .iter()
                .find(|set| set.ref_id == ref_id)
                .map(|set| set.frames.clone())
                .unwrap_or_default();
            let (want, got) = (lines(&want), lines(&got));
            if want != got {
                differences.push(format!(
                    "{name} {ref_id}:\n  grafana {want:#?}\n  daiya   {got:#?}"
                ));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "{} of {} scenarios differ:\n{}",
        differences.len(),
        scenarios.len(),
        differences.join("\n")
    );
}
