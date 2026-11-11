//! Conversion between the versions a kind is served at (CFM-07, PL-54, ADR-N-023, T-6716).
//!
//! Every kind is served at [`API_VERSION`]; `Pipeline` also at [`API_VERSION_V1ALPHA2`], its
//! newest. [`to_latest`] rewrites a manifest into the newest version of its kind, and
//! [`to_v1alpha1`] takes a Pipeline back where `v1alpha1` can hold it, so a manifest converts
//! both ways and an older Portal still reads what a newer one wrote.

use serde_json::{Map, Value};

use crate::envelope::{API_VERSION, API_VERSION_V1ALPHA2};
use crate::error::{Error, Result};

/// The newest version this platform serves `kind` at.
pub fn latest(kind: &str) -> &'static str {
    if kind == "Pipeline" {
        API_VERSION_V1ALPHA2
    } else {
        API_VERSION
    }
}

/// `body`, a whole manifest, rewritten into the newest version of its kind. A `v1alpha1`
/// Pipeline becomes the `v1alpha2` one it means: `source` → `sources: [source]`, `compute` →
/// `steps: [compute]`, `targetEndpoint` with its `output` → `outputs: [{ targetEndpoint, type,
/// mode }]`. Anything already in the second shape, mixed, or with no `source` (its input lives in
/// `bento.yaml`, which v1alpha2 has no place for) is left as it came, for validation to judge;
/// every other kind has one version and stays as it is.
pub fn to_latest(body: &mut Value) {
    if body.get("kind").and_then(Value::as_str) != Some("Pipeline") {
        return;
    }
    let Some(spec) = body.get_mut("spec").and_then(Value::as_object_mut) else {
        return;
    };
    let second = ["sources", "steps", "outputs"]
        .iter()
        .any(|key| spec.contains_key(*key));
    if second || !spec.contains_key("source") || !spec.contains_key("targetEndpoint") {
        return;
    }
    if let Some(source) = spec.remove("source") {
        spec.insert("sources".to_owned(), Value::Array(vec![source]));
    }
    if let Some(compute) = spec.remove("compute") {
        spec.insert("steps".to_owned(), Value::Array(vec![compute]));
    }
    let mut output = Map::new();
    if let Some(target) = spec.remove("targetEndpoint") {
        output.insert("targetEndpoint".to_owned(), target);
    }
    if let Some(Value::Object(written)) = spec.remove("output") {
        output.extend(written);
    }
    spec.insert(
        "outputs".to_owned(),
        Value::Array(vec![Value::Object(output)]),
    );
    body["apiVersion"] = Value::String(API_VERSION_V1ALPHA2.to_owned());
}

/// `body` taken back to `v1alpha1`, for a reader that serves only that version. A kind with one
/// version passes unchanged. A `v1alpha2` Pipeline converts when `v1alpha1` can hold it: one
/// source, at most one step that is a compute, one output and no routes; anything else is refused
/// naming the field `v1alpha1` has no place for, never dropped.
pub fn to_v1alpha1(body: &mut Value) -> Result<()> {
    if body.get("apiVersion").and_then(Value::as_str) != Some(API_VERSION_V1ALPHA2) {
        return Ok(());
    }
    let refuse = |field: &str, reason: &str| Error::Invalid {
        field: field.to_owned(),
        reason: format!("{reason}, which joinedcontext.com/v1alpha1 cannot hold (PL-54)"),
    };
    let Some(spec) = body.get_mut("spec").and_then(Value::as_object_mut) else {
        return Err(refuse("spec", "no spec"));
    };
    if spec
        .get("routes")
        .is_some_and(|r| r != &Value::Array(Vec::new()))
    {
        return Err(refuse("spec.routes", "routes"));
    }
    let mut one = |key: &str, at_most_none: bool| -> Result<Option<Value>> {
        match spec.remove(key) {
            None if at_most_none => Ok(None),
            Some(Value::Array(mut items)) if items.len() == 1 => Ok(items.pop()),
            Some(Value::Array(items)) if items.is_empty() && at_most_none => Ok(None),
            _ => Err(refuse(
                &format!("spec.{key}"),
                if at_most_none {
                    "more than one entry"
                } else {
                    "other than exactly one entry"
                },
            )),
        }
    };
    let source = one("sources", false)?;
    let compute = one("steps", true)?;
    let Some(Value::Object(mut output)) = one("outputs", false)? else {
        return Err(refuse("spec.outputs", "an output that is no object"));
    };
    let Some(target) = output.remove("targetEndpoint") else {
        return Err(refuse(
            "spec.outputs[0]",
            "an output without targetEndpoint",
        ));
    };
    spec.extend(source.map(|s| ("source".to_owned(), s)));
    spec.extend(compute.map(|c| ("compute".to_owned(), c)));
    spec.insert("targetEndpoint".to_owned(), target);
    if !output.is_empty() {
        spec.insert("output".to_owned(), Value::Object(output));
    }
    // A step that is a processor, not a compute, parses as no v1alpha1 `compute`.
    if serde_json::from_value::<crate::kinds::PipelineSpec>(Value::Object(spec.clone()))
        .ok()
        .filter(|parsed| !parsed.is_second_shape())
        .is_none()
    {
        return Err(refuse("spec.steps[0]", "a step that is no compute"));
    }
    body["apiVersion"] = Value::String(API_VERSION.to_owned());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v1alpha1_pipeline() -> Value {
        json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": { "name": "air", "namespace": "ovzdusie" },
            "spec": {
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air",
                "output": { "type": "AirQualityObserved", "mode": "upsert" }
            }
        })
    }

    /// CFM-07: a v1alpha1 Pipeline converts to v1alpha2, parses and validates there, and comes
    /// back byte-equal.
    #[test]
    fn a_v1alpha1_pipeline_converts_to_v1alpha2_and_round_trips() {
        let original = v1alpha1_pipeline();
        let mut body = original.clone();
        to_latest(&mut body);
        assert_eq!(body["apiVersion"], API_VERSION_V1ALPHA2);
        assert_eq!(body["spec"]["sources"][0]["dataSourceRef"]["name"], "mqtt");
        assert_eq!(body["spec"]["steps"][0]["kind"], "bloblang");
        assert_eq!(
            body["spec"]["outputs"][0],
            json!({ "targetEndpoint": "urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air",
                    "type": "AirQualityObserved", "mode": "upsert" })
        );
        let pipeline: crate::kinds::Pipeline =
            serde_json::from_value(body.clone()).expect("the converted manifest parses");
        pipeline.validate().expect("and validates at v1alpha2");

        to_v1alpha1(&mut body).expect("v1alpha1 holds it");
        assert_eq!(body, original);
        let back: crate::kinds::Pipeline = serde_json::from_value(body).expect("parses");
        back.validate().expect("and validates at v1alpha1");
    }

    /// CFM-07: what v1alpha1 cannot hold is refused naming the field, never dropped.
    #[test]
    fn a_v1alpha2_pipeline_v1alpha1_cannot_hold_is_refused_naming_the_field() {
        let mut latest = v1alpha1_pipeline();
        to_latest(&mut latest);
        let processor = json!([{ "kind": "processor", "processor": { "mapping": "root = this" } }]);
        let cases = [
            (
                "sources",
                json!([{ "dataSourceRef": { "kind": "DataSource", "name": "a" } },
                               { "dataSourceRef": { "kind": "DataSource", "name": "b" } }]),
            ),
            ("outputs", json!([])),
            ("routes", json!([{ "name": "hot" }])),
            ("steps", processor),
        ];
        for (key, value) in cases {
            let mut body = latest.clone();
            body["spec"][key] = value;
            let err = to_v1alpha1(&mut body).expect_err(key).to_string();
            assert!(err.contains(&format!("spec.{key}")), "{key}: {err}");
        }
    }

    /// CFM-07, one test per kind version: every kind but Pipeline has one version, which is its
    /// newest, and converts to itself both ways; a manifest already newest is left as it is.
    #[test]
    fn every_kind_version_converts_to_its_newest() {
        for info in crate::registry::KINDS {
            let mut body = json!({
                "apiVersion": API_VERSION,
                "kind": info.kind,
                "metadata": { "name": "x" },
                "spec": { "field": 1 }
            });
            let before = body.clone();
            to_latest(&mut body);
            assert!(
                crate::envelope::serves(info.kind, latest(info.kind)),
                "{}",
                info.kind
            );
            if info.kind == "Pipeline" {
                assert_eq!(body, before, "no source: left for validation to judge");
                continue;
            }
            assert_eq!(body, before, "{}", info.kind);
            to_v1alpha1(&mut body).expect("one version");
            assert_eq!(body, before, "{}", info.kind);
        }
        let mut newest = v1alpha1_pipeline();
        to_latest(&mut newest);
        let again = newest.clone();
        to_latest(&mut newest);
        assert_eq!(newest, again);
    }
}
