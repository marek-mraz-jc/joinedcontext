//! Stored Scripts in pipelines (PL-118…PL-121, T-7083).
//!
//! A Pipeline pins a published version of a `Script` by name and version, as a `script` step or
//! in `spec.scripts`. The judge refuses a pin of a version the Script does not keep or keeps as a
//! draft, and a Script change that edits a published version or drops one a pipeline pins
//! ([`check`]). The reconciler replaces each pin with one `javascript` processor holding the
//! version's code, after checking its SHA-256 ([`inline`]); a raw `javascript` step stays refused
//! by jc-core, so code reaches the runner only through a reviewed version. A Script that a
//! pipeline writing a public Endpoint pins is reviewed in the red lane ([`lane`]).

use crate::change::Lane;
use crate::error::ApiError;
use crate::judge::Ground;
use crate::resource::ResourceEnvelope;
use crate::store::Mirror;
use jc_core::kinds::{
    ApplyTo, ComputeKind, PipelineSpec, ProcessorStep, ScriptRef, ScriptSpec, ScriptState, Step,
};
use serde_json::{json, Value};

/// Every pin of `spec`, with the field it is written at.
pub fn pins(spec: &PipelineSpec) -> Vec<(String, ScriptRef)> {
    let step_pins = |at: String, steps: &[Step]| -> Vec<(String, ScriptRef)> {
        steps
            .iter()
            .enumerate()
            .filter_map(|(i, step)| match step {
                Step::Compute(c) => c
                    .script_ref
                    .clone()
                    .map(|r| (format!("{at}[{i}].scriptRef"), r)),
                Step::Processor(_) => None,
            })
            .collect()
    };
    let mut pins = step_pins("spec.steps".to_owned(), &spec.steps);
    for (r, route) in spec.routes.iter().enumerate() {
        pins.extend(step_pins(format!("spec.routes[{r}].steps"), &route.steps));
    }
    pins.extend(spec.scripts.iter().enumerate().map(|(i, applied)| {
        (
            format!("spec.scripts[{i}].scriptRef"),
            applied.script_ref.clone(),
        )
    }));
    pins
}

/// The Script `name` of `project` as it would stand: the Change's own copy first.
fn script_of(mirror: &Mirror, project: &str, name: &str) -> Option<ScriptSpec> {
    crate::references::bundled()
        .into_iter()
        .find(|m| {
            m["kind"] == "Script"
                && m["metadata"]["name"] == name
                && m["metadata"]["namespace"]
                    .as_str()
                    .is_none_or(|namespace| namespace == project)
        })
        .map(|m| m["spec"].clone())
        .or_else(|| mirror.get(project, "Script", name).map(|e| e.spec))
        .and_then(|spec| serde_json::from_value(spec).ok())
}

/// The project's Pipelines as they would stand, each with its name.
fn pipelines_of(mirror: &Mirror, project: &str) -> Vec<(String, PipelineSpec)> {
    let bundled: Vec<Value> = crate::references::bundled()
        .into_iter()
        .filter(|m| m["kind"] == "Pipeline")
        .collect();
    let held = mirror
        .kind_where(project, "Pipeline", |_| true)
        .into_iter()
        .filter(|e| {
            !bundled
                .iter()
                .any(|m| m["metadata"]["name"] == e.metadata.name)
        })
        .map(|e| (e.metadata.name, e.spec))
        .collect::<Vec<_>>();
    let proposed = bundled.into_iter().filter_map(|m| {
        Some((
            m["metadata"]["name"].as_str()?.to_owned(),
            m["spec"].clone(),
        ))
    });
    held.into_iter()
        .chain(proposed)
        .filter_map(|(name, spec)| Some((name, serde_json::from_value(spec).ok()?)))
        .collect()
}

fn refused(field: &str, detail: String) -> ApiError {
    ApiError::Unprocessable {
        detail,
        errors: vec![field.to_owned()],
    }
}

/// The judge's `scripts` check (PL-118, PL-119): a Pipeline's pins name published versions; a
/// Script keeps every published version as it stands on `main`, and every version a pipeline
/// pins.
pub fn check(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    match envelope.kind.as_str() {
        "Pipeline" => {
            let Ok(spec) = serde_json::from_value::<PipelineSpec>(envelope.spec.clone()) else {
                return Ok(());
            };
            for (field, pin) in pins(&spec) {
                // A Script that is not there at all is the references check's (MF-13).
                let Some(script) = script_of(ground.mirror, project, &pin.name) else {
                    continue;
                };
                match script.version(&pin.version) {
                    None => {
                        let kept: Vec<&str> =
                            script.versions.iter().map(|v| v.version.as_str()).collect();
                        return Err(refused(
                            &field,
                            format!(
                                "{field} names version {} of Script '{}', which keeps {}; pin one \
                                 of those (PL-119)",
                                pin.version,
                                pin.name,
                                kept.join(", ")
                            ),
                        ));
                    }
                    Some(v) if v.state == ScriptState::Draft => {
                        return Err(refused(
                            &field,
                            format!(
                                "{field} names version {} of Script '{}', which is a draft: \
                                 publish it through a Change first, or pin a published version \
                                 (PL-119)",
                                pin.version, pin.name
                            ),
                        ));
                    }
                    Some(_) => {}
                }
            }
            Ok(())
        }
        "Script" => {
            let name = &envelope.metadata.name;
            let Ok(proposed) = serde_json::from_value::<ScriptSpec>(envelope.spec.clone()) else {
                return Ok(());
            };
            if let Some(held) = ground
                .mirror
                .get(project, "Script", name)
                .and_then(|e| serde_json::from_value::<ScriptSpec>(e.spec).ok())
            {
                for old in held
                    .versions
                    .iter()
                    .filter(|v| v.state == ScriptState::Published)
                {
                    match proposed.version(&old.version) {
                        Some(new)
                            if (new.code != old.code
                                || new.sha256 != old.sha256
                                || new.limits != old.limits
                                || new.state != old.state) =>
                        {
                            return Err(refused(
                                "spec.versions",
                                format!(
                                    "version {} of Script '{name}' is published and never \
                                     changes: add the change as a new version, e.g. a higher \
                                     MINOR or PATCH (PL-118)",
                                    old.version
                                ),
                            ));
                        }
                        _ => {}
                    }
                }
            }
            for (pipeline, spec) in pipelines_of(ground.mirror, project) {
                for (field, pin) in pins(&spec) {
                    if pin.name == *name && proposed.version(&pin.version).is_none() {
                        return Err(refused(
                            "spec.versions",
                            format!(
                                "Pipeline '{pipeline}' pins version {} of Script '{name}' at \
                                 {field}: move its pin before removing the version (PL-118)",
                                pin.version
                            ),
                        ));
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The lane a Script change is reviewed in (PL-121): red when a pipeline that pins it writes
/// through a public Endpoint, otherwise what the kind alone says.
pub fn lane(mirror: &Mirror, project: &str, envelope: &ResourceEnvelope, lane: Lane) -> Lane {
    if envelope.kind != "Script" {
        return lane;
    }
    let public = pipelines_of(mirror, project).into_iter().any(|(_, spec)| {
        pins(&spec)
            .iter()
            .any(|(_, pin)| pin.name == envelope.metadata.name)
            && spec.outputs().iter().any(|output| {
                mirror
                    .get(project, "Endpoint", output.target_endpoint.local_id())
                    .is_some_and(|e| e.spec["audience"] == "public")
            })
    });
    if public {
        Lane::Red
    } else {
        lane
    }
}

/// One rendered `javascript` processor of a pin, after the digest check (PL-120).
fn processor(
    mirror: &Mirror,
    project: &str,
    field: &str,
    pin: &ScriptRef,
) -> Result<Value, String> {
    let script = mirror
        .get(project, "Script", &pin.name)
        .and_then(|e| serde_json::from_value::<ScriptSpec>(e.spec).ok())
        .ok_or_else(|| {
            format!(
                "{field} names Script '{}', which is not in the project",
                pin.name
            )
        })?;
    let version = script
        .version(&pin.version)
        .filter(|v| v.state == ScriptState::Published)
        .ok_or_else(|| {
            format!(
                "{field} names version {} of Script '{}', which is not published",
                pin.version, pin.name
            )
        })?;
    if !version.intact() {
        return Err(format!(
            "the code of Script '{}' version {} does not match its sha256: nothing is rendered \
             until the Script is fixed through a Change (PL-120)",
            pin.name, pin.version
        ));
    }
    let limits = version.limits.unwrap_or_default();
    Ok(json!({
        "label": format!("script_{}_{}", pin.name.replace('-', "_"), &version.sha256[..12]),
        "javascript": {
            "code": version.code,
            "timeout_ms": limits.timeout_ms(),
            "memory_mb": limits.memory_mb(),
        }
    }))
}

fn step(processor: Value) -> Step {
    let processor = processor
        .as_object()
        .map(|o| o.clone().into_iter().collect())
        .unwrap_or_default();
    Step::Processor(ProcessorStep { processor })
}

/// Only for records of `types`, when it names any: Bento's `switch` over the record's `type`.
fn for_types(processor: Value, types: &[String]) -> Value {
    if types.is_empty() {
        return processor;
    }
    let names: Vec<Value> = types.iter().map(|t| Value::String(t.clone())).collect();
    json!({ "switch": [{
        "check": format!("{}.contains(this.type)", Value::Array(names)),
        "processors": [processor],
    }] })
}

/// Replaces every pin of `spec` with the `javascript` processor of its version (PL-120): a
/// `script` step in place, `spec.scripts` at the start or the end of `spec.steps`. A pin that
/// does not resolve or whose code does not match its digest is an error, and the caller renders
/// no stream for the pipeline.
pub fn inline(mirror: &Mirror, project: &str, spec: &mut PipelineSpec) -> Result<(), String> {
    let inline_steps = |at: &str, steps: &mut Vec<Step>| -> Result<(), String> {
        for (i, step_at) in steps.iter_mut().enumerate() {
            if let Step::Compute(c) = step_at {
                if c.kind == ComputeKind::Script {
                    let pin = c
                        .script_ref
                        .clone()
                        .ok_or_else(|| format!("{at}[{i}] is a script step without scriptRef"))?;
                    *step_at = step(processor(
                        mirror,
                        project,
                        &format!("{at}[{i}].scriptRef"),
                        &pin,
                    )?);
                }
            }
        }
        Ok(())
    };
    inline_steps("spec.steps", &mut spec.steps)?;
    for (r, route) in spec.routes.iter_mut().enumerate() {
        inline_steps(&format!("spec.routes[{r}].steps"), &mut route.steps)?;
    }
    let mut start = Vec::new();
    for (i, applied) in std::mem::take(&mut spec.scripts).into_iter().enumerate() {
        let rendered = processor(
            mirror,
            project,
            &format!("spec.scripts[{i}].scriptRef"),
            &applied.script_ref,
        )?;
        let rendered = step(for_types(rendered, &applied.types));
        match applied.apply_to {
            ApplyTo::StepsStart => start.push(rendered),
            ApplyTo::BeforeOutput => spec.steps.push(rendered),
        }
    }
    spec.steps.splice(0..0, start);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ResourceEnvelope;
    use jc_core::kinds::OrganizationBounds;

    const V1: &str =
        "let m = bento.v0_msg_as_structured(); m.v = 1; bento.v0_msg_set_structured(m);";
    const V2: &str =
        "let m = bento.v0_msg_as_structured(); m.v = 2; bento.v0_msg_set_structured(m);";

    fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": kind,
            "metadata": { "name": name, "namespace": "helsinki" },
            "spec": spec
        }))
        .expect("envelope")
    }

    fn version(v: &str, state: &str, code: &str) -> Value {
        json!({ "version": v, "state": state, "code": code,
                "sha256": jc_core::kinds::script::digest(code) })
    }

    fn script(versions: Value) -> ResourceEnvelope {
        envelope(
            "Script",
            "flag",
            json!({ "language": "javascript", "versions": versions }),
        )
    }

    fn pipeline_spec(pin: &str, extra: Value) -> Value {
        let mut spec = json!({
            "class": "resident",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
            "steps": [
                { "kind": "bloblang", "bloblang": "root = this" },
                { "kind": "script", "scriptRef": { "name": "flag", "version": pin } }
            ],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:intake" }]
        });
        spec.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        spec
    }

    fn judged(mirror: &Mirror, proposed: &ResourceEnvelope) -> Result<(), ApiError> {
        let bounds = OrganizationBounds::default();
        let ground = Ground {
            mirror,
            bounds: &bounds,
            apex: None,
            domain: None,
            portals: None,
        };
        check(&ground, "helsinki", proposed)
    }

    fn field_of(err: ApiError) -> Vec<String> {
        match err {
            ApiError::Unprocessable { errors, .. } => errors,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn rendered(mirror: &Mirror, spec: Value) -> Result<PipelineSpec, String> {
        let mut spec: PipelineSpec = serde_json::from_value(spec).expect("pipeline");
        inline(mirror, "helsinki", &mut spec).map(|()| spec)
    }

    fn code_at(spec: &PipelineSpec, i: usize) -> String {
        match &spec.steps[i] {
            Step::Processor(p) => p.processor["javascript"]["code"]
                .as_str()
                .expect("code")
                .to_owned(),
            other => panic!("not a processor: {other:?}"),
        }
    }

    /// PL-119/PL-120: a pipeline pins 1.0.0; publishing 1.1.0 leaves it on 1.0.0 until its pin
    /// moves.
    #[test]
    fn a_new_version_changes_nothing_until_the_pin_moves() {
        let mirror = Mirror::new();
        mirror.upsert(script(json!([version("1.0.0", "published", V1)])));
        assert_eq!(
            code_at(
                &rendered(&mirror, pipeline_spec("1.0.0", json!({}))).expect("ok"),
                1
            ),
            V1
        );
        mirror.upsert(script(json!([
            version("1.0.0", "published", V1),
            version("1.1.0", "published", V2)
        ])));
        let pinned = rendered(&mirror, pipeline_spec("1.0.0", json!({}))).expect("ok");
        assert_eq!(code_at(&pinned, 1), V1);
        let Step::Processor(p) = &pinned.steps[1] else {
            panic!("processor")
        };
        assert_eq!(p.processor["javascript"]["timeout_ms"], 50);
        assert!(p.processor["label"]
            .as_str()
            .expect("label")
            .starts_with("script_flag_"));
        assert_eq!(
            code_at(
                &rendered(&mirror, pipeline_spec("1.1.0", json!({}))).expect("ok"),
                1
            ),
            V2
        );
    }

    /// PL-120: code that no longer matches its digest renders nothing.
    #[test]
    fn a_tampered_version_renders_no_processor() {
        let mirror = Mirror::new();
        let mut tampered = version("1.0.0", "published", V1);
        tampered["code"] = json!(V2);
        // The mirror takes what the forge holds; jc-core would refuse this file at load.
        mirror.upsert(script(json!([tampered])));
        let err = rendered(&mirror, pipeline_spec("1.0.0", json!({}))).expect_err("refused");
        assert!(err.contains("does not match its sha256"), "{err}");
    }

    /// PL-119: applied scripts run first or last, and only for their types.
    #[test]
    fn applied_scripts_run_at_the_start_or_before_output() {
        let mirror = Mirror::new();
        mirror.upsert(script(json!([version("1.0.0", "published", V1)])));
        let spec = rendered(
            &mirror,
            pipeline_spec(
                "1.0.0",
                json!({ "scripts": [
                    { "scriptRef": { "name": "flag", "version": "1.0.0" }, "applyTo": "before-output",
                      "types": ["ParkingSpot"] },
                    { "scriptRef": { "name": "flag", "version": "1.0.0" }, "applyTo": "steps-start" }
                ] }),
            ),
        )
        .expect("ok");
        assert_eq!(spec.steps.len(), 4);
        assert!(spec.scripts.is_empty());
        assert_eq!(code_at(&spec, 0), V1);
        let Step::Processor(last) = &spec.steps[3] else {
            panic!("processor")
        };
        assert_eq!(
            last.processor["switch"][0]["check"],
            r#"["ParkingSpot"].contains(this.type)"#
        );
        assert!(matches!(&spec.steps[1], Step::Compute(c) if c.kind == ComputeKind::Bloblang));
    }

    /// PL-119: the judge refuses a pin of a draft and of a version the Script does not keep.
    #[test]
    fn a_pin_of_a_draft_or_an_unknown_version_is_refused() {
        let mirror = Mirror::new();
        mirror.upsert(script(json!([
            version("1.0.0", "published", V1),
            version("1.1.0", "draft", V2)
        ])));
        let draft = judged(
            &mirror,
            &envelope("Pipeline", "p", pipeline_spec("1.1.0", json!({}))),
        )
        .expect_err("draft");
        assert!(format!("{draft:?}").contains("is a draft"));
        assert_eq!(field_of(draft), vec!["spec.steps[1].scriptRef"]);
        let unknown = judged(
            &mirror,
            &envelope("Pipeline", "p", pipeline_spec("2.0.0", json!({}))),
        )
        .expect_err("unknown");
        assert!(format!("{unknown:?}").contains("keeps 1.0.0, 1.1.0"));
        judged(
            &mirror,
            &envelope("Pipeline", "p", pipeline_spec("1.0.0", json!({}))),
        )
        .expect("published");
    }

    /// PL-118: a published version never changes, and a pinned one never goes.
    #[test]
    fn a_published_version_is_frozen_and_a_pinned_one_stays() {
        let mirror = Mirror::new();
        mirror.upsert(script(json!([version("1.0.0", "published", V1)])));
        mirror.upsert(envelope("Pipeline", "p", pipeline_spec("1.0.0", json!({}))));
        let edited = judged(&mirror, &script(json!([version("1.0.0", "published", V2)])))
            .expect_err("edited");
        assert!(format!("{edited:?}").contains("never changes"));
        let dropped = judged(&mirror, &script(json!([version("1.1.0", "published", V2)])))
            .expect_err("dropped");
        assert!(format!("{dropped:?}").contains("Pipeline 'p' pins version 1.0.0"));
        judged(
            &mirror,
            &script(json!([
                version("1.0.0", "published", V1),
                version("1.1.0", "draft", V2)
            ])),
        )
        .expect("a new version beside it");
    }

    /// PL-121: a Script a pipeline writing a public Endpoint pins is red; otherwise its lane.
    #[test]
    fn a_script_feeding_a_public_endpoint_is_red() {
        let mirror = Mirror::new();
        mirror.upsert(envelope("Pipeline", "p", pipeline_spec("1.0.0", json!({}))));
        let proposed = script(json!([version("1.0.0", "published", V1)]));
        mirror.upsert(envelope(
            "Endpoint",
            "intake",
            json!({ "audience": "organization" }),
        ));
        assert_eq!(
            lane(&mirror, "helsinki", &proposed, Lane::Yellow),
            Lane::Yellow
        );
        mirror.upsert(envelope(
            "Endpoint",
            "intake",
            json!({ "audience": "public" }),
        ));
        assert_eq!(
            lane(&mirror, "helsinki", &proposed, Lane::Yellow),
            Lane::Red
        );
    }
}
