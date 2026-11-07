//! Structural resource diffing and RFC 7386 JSON merge patching (MF-13, R17).
//!
//! Provides the reviewer-facing delta between current and desired resource envelopes
//! before proposed changes are merged into Git and converged by the reconciler.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::change::PlanSummary;
use crate::resource::ResourceEnvelope;

/// Summary and field-level changes between two resource revisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanDiff {
    pub summary: PlanSummary,
    pub fields: Vec<FieldChange>,
}

impl PlanDiff {
    pub fn empty() -> Self {
        Self {
            summary: PlanSummary::default(),
            fields: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

impl Default for PlanDiff {
    fn default() -> Self {
        Self::empty()
    }
}

/// A single leaf field modification in a plan diff.
///
/// Paths are dotted strings rooted at `metadata` or `spec` (e.g. `spec.audience`,
/// `spec.representations[1]`, `metadata.labels.joinedcontext.com/domain`).
/// Keys containing dots (such as reverse-DNS label names) are deliberately not escaped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub from: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub to: Option<serde_json::Value>,
}

/// Whether applying this change makes the runner restart the pipeline's stream (T-1056, PL-45).
///
/// The reconciler sends a stream to the runner again whenever its rendered configuration moves,
/// and the runner restarts a stream on every PUT — which for a periodic pipeline means its
/// schedule starts over, so the next emission is a period away rather than where it was. The
/// render is a function of the pipeline's `spec`, so a change that touches the spec restarts it
/// and a change to metadata alone does not. `spec.enabled` is the exception: it takes the stream
/// away or brings it back, which the person toggling it already knows, so it warns about nothing.
pub fn restarts_stream(kind: &str, diff: &PlanDiff) -> bool {
    kind == "Pipeline"
        && diff.fields.iter().any(|field| {
            (field.path == "spec" || field.path.starts_with("spec."))
                && field.path != "spec.enabled"
        })
}

/// Computes the structural diff between current and desired resource envelopes.
///
/// Compares `metadata` and `spec` only; `status` is never part of a plan (MF-04).
/// Keys containing dots are not escaped in paths.
pub fn diff(current: Option<&ResourceEnvelope>, desired: Option<&ResourceEnvelope>) -> PlanDiff {
    match (current, desired) {
        (None, None) => PlanDiff::empty(),
        (None, Some(des)) => {
            let fields = envelope_leaves(des)
                .into_iter()
                .map(|(path, val)| FieldChange {
                    path,
                    from: None,
                    to: Some(val),
                })
                .collect();
            PlanDiff {
                summary: PlanSummary::new(1, 0, 0),
                fields,
            }
        }
        (Some(curr), None) => {
            let fields = envelope_leaves(curr)
                .into_iter()
                .map(|(path, val)| FieldChange {
                    path,
                    from: Some(val),
                    to: None,
                })
                .collect();
            PlanDiff {
                summary: PlanSummary::new(0, 0, 1),
                fields,
            }
        }
        (Some(curr), Some(des)) => {
            let mut fields = Vec::new();
            let curr_meta = serde_json::to_value(&curr.metadata).unwrap_or(serde_json::Value::Null);
            let des_meta = serde_json::to_value(&des.metadata).unwrap_or(serde_json::Value::Null);

            diff_values("metadata", &curr_meta, &des_meta, &mut fields);
            diff_values("spec", &curr.spec, &des.spec, &mut fields);

            let update = if fields.is_empty() { 0 } else { 1 };
            PlanDiff {
                summary: PlanSummary::new(0, update, 0),
                fields,
            }
        }
    }
}

/// Collects all leaf values from `metadata` and `spec` of a resource envelope.
fn envelope_leaves(env: &ResourceEnvelope) -> Vec<(String, serde_json::Value)> {
    let mut leaves = Vec::new();
    if let Ok(meta_val) = serde_json::to_value(&env.metadata) {
        collect_leaves("metadata", &meta_val, &mut leaves);
    }
    collect_leaves("spec", &env.spec, &mut leaves);
    leaves
}

/// Recursively traverses a JSON value to collect all leaf paths and values.
fn collect_leaves(
    prefix: &str,
    val: &serde_json::Value,
    leaves: &mut Vec<(String, serde_json::Value)>,
) {
    match val {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                if prefix != "metadata" && prefix != "spec" {
                    leaves.push((prefix.to_string(), val.clone()));
                }
            } else {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by_key(|(k1, _)| *k1);
                for (k, v) in entries {
                    let path = format!("{prefix}.{k}");
                    collect_leaves(&path, v, leaves);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                leaves.push((prefix.to_string(), val.clone()));
            } else {
                for (i, v) in arr.iter().enumerate() {
                    let path = format!("{prefix}[{i}]");
                    collect_leaves(&path, v, leaves);
                }
            }
        }
        _ => {
            leaves.push((prefix.to_string(), val.clone()));
        }
    }
}

/// Recursively computes leaf-level changes between two JSON values.
fn diff_values(
    path: &str,
    curr: &serde_json::Value,
    des: &serde_json::Value,
    fields: &mut Vec<FieldChange>,
) {
    if curr == des {
        return;
    }

    match (curr, des) {
        (serde_json::Value::Object(curr_map), serde_json::Value::Object(des_map)) => {
            let keys: BTreeSet<_> = curr_map.keys().chain(des_map.keys()).collect();
            for key in keys {
                let child_path = format!("{path}.{key}");
                match (curr_map.get(key), des_map.get(key)) {
                    (Some(c), Some(d)) => diff_values(&child_path, c, d, fields),
                    (Some(c), None) => {
                        let mut removed = Vec::new();
                        collect_leaves(&child_path, c, &mut removed);
                        for (p, val) in removed {
                            fields.push(FieldChange {
                                path: p,
                                from: Some(val),
                                to: None,
                            });
                        }
                    }
                    (None, Some(d)) => {
                        let mut added = Vec::new();
                        collect_leaves(&child_path, d, &mut added);
                        for (p, val) in added {
                            fields.push(FieldChange {
                                path: p,
                                from: None,
                                to: Some(val),
                            });
                        }
                    }
                    (None, None) => {}
                }
            }
        }
        (serde_json::Value::Array(curr_arr), serde_json::Value::Array(des_arr)) => {
            let min_len = usize::min(curr_arr.len(), des_arr.len());
            for i in 0..min_len {
                let elem_path = format!("{path}[{i}]");
                diff_values(&elem_path, &curr_arr[i], &des_arr[i], fields);
            }
            for (i, item) in curr_arr.iter().enumerate().skip(min_len) {
                let elem_path = format!("{path}[{i}]");
                let mut removed = Vec::new();
                collect_leaves(&elem_path, item, &mut removed);
                for (p, val) in removed {
                    fields.push(FieldChange {
                        path: p,
                        from: Some(val),
                        to: None,
                    });
                }
            }
            for (i, item) in des_arr.iter().enumerate().skip(min_len) {
                let elem_path = format!("{path}[{i}]");
                let mut added = Vec::new();
                collect_leaves(&elem_path, item, &mut added);
                for (p, val) in added {
                    fields.push(FieldChange {
                        path: p,
                        from: None,
                        to: Some(val),
                    });
                }
            }
        }
        _ => {
            fields.push(FieldChange {
                path: path.to_string(),
                from: Some(curr.clone()),
                to: Some(des.clone()),
            });
        }
    }
}

/// Applies an RFC 7386 JSON Merge Patch to a target value in-place.
///
/// When both target and patch are objects, keys are merged recursively;
/// a `null` value in the patch removes the corresponding key from the target.
/// Any other patch type (scalar, array, or replacing a non-object) replaces
/// the target outright.
pub fn merge_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
    if let serde_json::Value::Object(patch_obj) = patch {
        if !target.is_object() {
            *target = serde_json::Value::Object(serde_json::Map::new());
        }
        if let serde_json::Value::Object(target_obj) = target {
            for (key, value) in patch_obj {
                if value.is_null() {
                    target_obj.remove(key);
                } else {
                    let entry = target_obj
                        .entry(key.clone())
                        .or_insert(serde_json::Value::Null);
                    merge_patch(entry, value);
                }
            }
        }
    } else {
        *target = patch.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, Phase, Status, API_VERSION};
    use serde_json::json;

    fn sample_envelope(name: &str, spec: serde_json::Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_string(),
            kind: "Endpoint".to_string(),
            metadata: ObjectMeta {
                name: name.to_string(),
                namespace: Some("ovzdusie".to_string()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    #[test]
    fn diff_none_none_is_empty() {
        let plan = diff(None, None);
        assert_eq!(plan.summary, PlanSummary::new(0, 0, 0));
        assert!(plan.fields.is_empty());
        assert!(plan.is_empty());
    }

    #[test]
    fn diff_create() {
        let env = sample_envelope("public-air", json!({ "audience": "public" }));
        let plan = diff(None, Some(&env));

        assert_eq!(plan.summary, PlanSummary::new(1, 0, 0));
        assert!(!plan.fields.is_empty());

        for f in &plan.fields {
            assert!(f.from.is_none());
            assert!(f.to.is_some());
        }

        let paths: Vec<&str> = plan.fields.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"metadata.name"));
        assert!(paths.contains(&"metadata.namespace"));
        assert!(paths.contains(&"spec.audience"));
    }

    #[test]
    fn diff_delete() {
        let env = sample_envelope("public-air", json!({ "audience": "public" }));
        let plan = diff(Some(&env), None);

        assert_eq!(plan.summary, PlanSummary::new(0, 0, 1));
        assert!(!plan.fields.is_empty());

        for f in &plan.fields {
            assert!(f.from.is_some());
            assert!(f.to.is_none());
        }

        let paths: Vec<&str> = plan.fields.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"metadata.name"));
        assert!(paths.contains(&"metadata.namespace"));
        assert!(paths.contains(&"spec.audience"));
    }

    #[test]
    fn diff_nested_spec_update() {
        let curr = sample_envelope(
            "public-air",
            json!({
                "config": {
                    "timeout": 30,
                    "mode": "strict"
                }
            }),
        );
        let des = sample_envelope(
            "public-air",
            json!({
                "config": {
                    "timeout": 60,
                    "mode": "strict"
                }
            }),
        );

        let plan = diff(Some(&curr), Some(&des));
        assert_eq!(plan.summary, PlanSummary::new(0, 1, 0));
        assert_eq!(plan.fields.len(), 1);
        assert_eq!(plan.fields[0].path, "spec.config.timeout");
        assert_eq!(plan.fields[0].from, Some(json!(30)));
        assert_eq!(plan.fields[0].to, Some(json!(60)));
    }

    #[test]
    fn diff_array_element_change() {
        let curr = sample_envelope(
            "public-air",
            json!({
                "representations": ["ngsi-ld", "file.geojson"]
            }),
        );
        let des = sample_envelope(
            "public-air",
            json!({
                "representations": ["ngsi-ld", "file.csv"]
            }),
        );

        let plan = diff(Some(&curr), Some(&des));
        assert_eq!(plan.summary, PlanSummary::new(0, 1, 0));
        assert_eq!(plan.fields.len(), 1);
        assert_eq!(plan.fields[0].path, "spec.representations[1]");
        assert_eq!(plan.fields[0].from, Some(json!("file.geojson")));
        assert_eq!(plan.fields[0].to, Some(json!("file.csv")));
    }

    #[test]
    fn diff_array_that_grew() {
        let curr = sample_envelope(
            "public-air",
            json!({
                "representations": ["ngsi-ld"]
            }),
        );
        let des = sample_envelope(
            "public-air",
            json!({
                "representations": ["ngsi-ld", "file.geojson"]
            }),
        );

        let plan = diff(Some(&curr), Some(&des));
        assert_eq!(plan.summary, PlanSummary::new(0, 1, 0));
        assert_eq!(plan.fields.len(), 1);
        assert_eq!(plan.fields[0].path, "spec.representations[1]");
        assert_eq!(plan.fields[0].from, None);
        assert_eq!(plan.fields[0].to, Some(json!("file.geojson")));
    }

    #[test]
    fn diff_unchanged_pair() {
        let curr = sample_envelope("public-air", json!({ "audience": "public" }));
        let des = sample_envelope("public-air", json!({ "audience": "public" }));

        let plan = diff(Some(&curr), Some(&des));
        assert_eq!(plan.summary, PlanSummary::new(0, 0, 0));
        assert!(plan.fields.is_empty());
    }

    #[test]
    fn diff_status_differences_alone_produce_empty_diff() {
        let mut curr = sample_envelope("public-air", json!({ "audience": "public" }));
        curr.status = Some(Status {
            phase: Phase::Draft,
            observed_revision: Some("rev-1".into()),
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        });
        let mut des = sample_envelope("public-air", json!({ "audience": "public" }));
        des.status = Some(Status {
            phase: Phase::Live,
            observed_revision: Some("rev-2".into()),
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        });

        let plan = diff(Some(&curr), Some(&des));
        assert_eq!(plan.summary, PlanSummary::new(0, 0, 0));
        assert!(plan.fields.is_empty());
    }

    #[test]
    fn diff_labels_with_dots_are_not_escaped() {
        let mut curr = sample_envelope("public-air", json!({}));
        curr.metadata
            .labels
            .insert("joinedcontext.com/domain".into(), "environment".into());
        let mut des = sample_envelope("public-air", json!({}));
        des.metadata
            .labels
            .insert("joinedcontext.com/domain".into(), "mobility".into());

        let plan = diff(Some(&curr), Some(&des));
        assert_eq!(plan.summary, PlanSummary::new(0, 1, 0));
        assert_eq!(plan.fields.len(), 1);
        assert_eq!(
            plan.fields[0].path,
            "metadata.labels.joinedcontext.com/domain"
        );
        assert_eq!(plan.fields[0].from, Some(json!("environment")));
        assert_eq!(plan.fields[0].to, Some(json!("mobility")));
    }

    #[test]
    fn merge_patch_recursive_object_merge() {
        let mut target = json!({
            "title": "Old",
            "author": {
                "givenName": "John",
                "familyName": "Doe"
            }
        });
        let patch = json!({
            "title": "New",
            "author": {
                "familyName": "Smith"
            }
        });

        merge_patch(&mut target, &patch);
        assert_eq!(
            target,
            json!({
                "title": "New",
                "author": {
                    "givenName": "John",
                    "familyName": "Smith"
                }
            })
        );
    }

    #[test]
    fn merge_patch_null_deletes_key() {
        let mut target = json!({
            "a": "b",
            "c": "d"
        });
        let patch = json!({
            "a": null
        });

        merge_patch(&mut target, &patch);
        assert_eq!(target, json!({ "c": "d" }));
    }

    #[test]
    fn merge_patch_scalar_replaces_object() {
        let mut target = json!({
            "author": {
                "name": "Alice"
            }
        });
        let patch = json!({
            "author": "Alice"
        });

        merge_patch(&mut target, &patch);
        assert_eq!(target, json!({ "author": "Alice" }));
    }

    #[test]
    fn merge_patch_patch_not_an_object_replaces_target() {
        let mut target = json!({ "key": "value" });
        let patch = json!("just a string");
        merge_patch(&mut target, &patch);
        assert_eq!(target, json!("just a string"));

        let mut target2 = json!({ "a": 1 });
        let patch2 = json!([1, 2, 3]);
        merge_patch(&mut target2, &patch2);
        assert_eq!(target2, json!([1, 2, 3]));
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;

    fn changed(paths: &[&str]) -> PlanDiff {
        PlanDiff {
            summary: PlanSummary::default(),
            fields: paths
                .iter()
                .map(|path| FieldChange {
                    path: (*path).to_owned(),
                    from: None,
                    to: None,
                })
                .collect(),
        }
    }

    /// T-1056, PL-45: the runner restarts a stream on every PUT, and the reconciler sends one
    /// whenever the render moves. The render is a function of the pipeline's `spec`, so a person
    /// is told before approving that a periodic pipeline's schedule starts over.
    #[test]
    fn a_pipeline_whose_spec_moves_restarts_its_stream() {
        assert!(restarts_stream(
            "Pipeline",
            &changed(&["spec.compute.bloblang"])
        ));
        assert!(restarts_stream("Pipeline", &changed(&["spec"])));
        assert!(restarts_stream(
            "Pipeline",
            &changed(&["metadata.title", "spec.output.mode"])
        ));
    }

    #[test]
    fn a_change_that_leaves_the_spec_alone_does_not() {
        assert!(!restarts_stream("Pipeline", &changed(&["metadata.title"])));
        assert!(!restarts_stream("Pipeline", &PlanDiff::empty()));
        // A field merely beginning with the letters is not the spec.
        assert!(!restarts_stream("Pipeline", &changed(&["specification"])));
        // Turning a pipeline off takes its stream away; nobody needs to be told that as a surprise.
        assert!(!restarts_stream("Pipeline", &changed(&["spec.enabled"])));
        // R2POR5-016: a label edit beside the toggle restarts nothing either.
        assert!(!restarts_stream(
            "Pipeline",
            &changed(&["spec.enabled", "metadata.labels.x"])
        ));
        // But a spec change riding along with the toggle still restarts what stays.
        assert!(restarts_stream(
            "Pipeline",
            &changed(&["spec.enabled", "spec.compute.bloblang"])
        ));
    }

    /// Only a pipeline has a stream to restart.
    #[test]
    fn another_kind_never_restarts_a_stream() {
        for kind in ["Endpoint", "ContextSpace", "DataSource", "Policy"] {
            assert!(
                !restarts_stream(kind, &changed(&["spec.anything"])),
                "{kind}"
            );
        }
    }
}

/// Which side a person kept for one conflicting field (CC-80).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Ours,
    Theirs,
}

/// One field both sides changed to different values since their common base (CC-80).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FieldConflict {
    pub path: String,
    #[schema(value_type = Object)]
    pub ours: serde_json::Value,
    #[schema(value_type = Object)]
    pub theirs: serde_json::Value,
    #[schema(value_type = Object)]
    pub base: serde_json::Value,
}

/// A three-way merge of two JSON documents with their common base (CC-80).
///
/// A field only one side changed takes that side's value; a field both changed alike takes it
/// once. A field both changed differently is a conflict, and `pick` names the side a person
/// kept; with no answer the merge fails listing every such field, so no side wins by default.
/// Objects merge key by key; an array is one value, like a scalar. Paths read as the plan's:
/// `spec.rateLimit.perMinute`.
pub fn merge3(
    base: Option<&serde_json::Value>,
    ours: &serde_json::Value,
    theirs: &serde_json::Value,
    pick: &dyn Fn(&str) -> Option<Side>,
) -> Result<serde_json::Value, Vec<FieldConflict>> {
    let mut conflicts = Vec::new();
    let merged = merge_at("", base, Some(ours), Some(theirs), pick, &mut conflicts);
    if conflicts.is_empty() {
        Ok(merged.unwrap_or(serde_json::Value::Null))
    } else {
        Err(conflicts)
    }
}

fn merge_at(
    path: &str,
    base: Option<&serde_json::Value>,
    ours: Option<&serde_json::Value>,
    theirs: Option<&serde_json::Value>,
    pick: &dyn Fn(&str) -> Option<Side>,
    conflicts: &mut Vec<FieldConflict>,
) -> Option<serde_json::Value> {
    use serde_json::Value;
    if ours == theirs || theirs == base {
        return ours.cloned();
    }
    if ours == base {
        return theirs.cloned();
    }
    if let (Some(Value::Object(o)), Some(Value::Object(t))) = (ours, theirs) {
        let b = base.and_then(Value::as_object);
        let keys: std::collections::BTreeSet<&String> = o.keys().chain(t.keys()).collect();
        let mut out = serde_json::Map::new();
        for key in keys {
            let child = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            let merged = merge_at(
                &child,
                b.and_then(|b| b.get(key)),
                o.get(key),
                t.get(key),
                pick,
                conflicts,
            );
            if let Some(value) = merged {
                out.insert(key.clone(), value);
            }
        }
        return Some(Value::Object(out));
    }
    match pick(path) {
        Some(Side::Ours) => ours.cloned(),
        Some(Side::Theirs) => theirs.cloned(),
        None => {
            conflicts.push(FieldConflict {
                path: path.to_owned(),
                ours: ours.cloned().unwrap_or(Value::Null),
                theirs: theirs.cloned().unwrap_or(Value::Null),
                base: base.cloned().unwrap_or(Value::Null),
            });
            ours.cloned()
        }
    }
}
