//! A Context Space's governance readings through the registry (JC2-60): the daily data-quality
//! report and the checks on write, the entity count, how pipelines create history, which
//! standard surfaces the model can serve, and the drift the last scan found.
//!
//! These are readings about the data, not manifests, and each had a browser route only. One
//! operation names the reading by `aspect` rather than six tools a model re-reads every turn; each
//! aspect calls the route's own function, so the grants and the answers stay the route's.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, NAME};
use super::runs::as_user;
use super::{annotations, parse_input, OpError, Operation};
use crate::api::temporal_audit::AuditQuery;
use crate::change::Lane;

pub const GOVERNANCE: &str = "jc_space_governance";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Aspect {
    Quality,
    QualityChecks,
    Usage,
    TemporalAudit,
    SurfaceFit,
    Drift,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GovernanceInput {
    pub space: String,
    pub aspect: Aspect,
    /// temporalAudit: only this pipeline.
    #[serde(default)]
    pub pipeline: Option<String>,
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": text("Context Space name, e.g. air", NAME),
            "aspect": {
                "type": "string",
                "enum": ["quality", "qualityChecks", "usage", "temporalAudit", "surfaceFit", "drift"],
                "description": "quality: daily report; qualityChecks: checks on write, 24 h and 7 d; usage: entity count; temporalAudit: how pipelines create history; surfaceFit: standard surfaces the model serves; drift: live differences from the repository"
            },
            "pipeline": text("temporalAudit: only this pipeline", NAME)
        },
        "required": ["space", "aspect"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    json!({ "type": "object", "description": "The reading, as the space's page shows it" })
}

fn validate(val: &Value) -> Result<(), OpError> {
    let input: GovernanceInput = parse_input(val.clone())?;
    if input.pipeline.is_some() && input.aspect != Aspect::TemporalAudit {
        return Err(OpError::InvalidInput {
            path: "/pipeline".into(),
            message: "pipeline narrows only the temporalAudit aspect".into(),
        });
    }
    Ok(())
}

/// The drift of one space: the scan answers per project, each finding naming its space.
fn of_space(mut found: Value, space: &str) -> Value {
    if let Some(items) = found.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(|item| item["space"] == space);
    }
    found
}

async fn read(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: GovernanceInput,
) -> Result<Value, OpError> {
    let user = || as_user(caller);
    let api = || State(state.clone());
    let at = || Path((project.to_owned(), input.space.clone()));
    use crate::api;
    Ok(match input.aspect {
        Aspect::Quality => api::quality::get_quality(user(), api(), at()).await?.0,
        Aspect::QualityChecks => {
            serde_json::to_value(api::quality::get_checks(user(), api(), at()).await?.0)?
        }
        Aspect::Usage => {
            serde_json::to_value(api::space_usage::get_usage(user(), api(), at()).await?.0)?
        }
        Aspect::TemporalAudit => {
            let query = Query(AuditQuery {
                pipeline: input.pipeline.clone(),
            });
            serde_json::to_value(
                api::temporal_audit::temporal_audit(user(), api(), at(), query)
                    .await?
                    .0,
            )?
        }
        Aspect::SurfaceFit => {
            serde_json::to_value(api::surface_fit::surface_fit(user(), api(), at()).await?.0)?
        }
        Aspect::Drift => {
            let Json(found) =
                api::drift::list_drift(api(), user(), Path(project.to_owned())).await?;
            of_space(found, &input.space)
        }
    })
}

pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: GOVERNANCE,
        title: "Read Space Governance",
        description: "One reading about a Context Space's data: quality, checks on write, entity count, history creation, surface fit or drift",
        input: input_schema,
        output: output_schema,
        annotations: annotations(true, false, true),
        kind: "ContextSpace",
        verb: None,
        lane: Lane::Green,
        validate,
        run: |caller, state, project, val| {
            Box::pin(async move {
                validate(&val)?;
                read(caller, state, project, parse_input(val)?).await
            })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aspect_is_one_of_six_and_pipeline_narrows_only_the_audit() {
        assert!(validate(&json!({ "space": "air", "aspect": "quality" })).is_ok());
        assert!(
            validate(&json!({ "space": "air", "aspect": "temporalAudit", "pipeline": "p" }))
                .is_ok()
        );
        assert!(validate(&json!({ "space": "air", "aspect": "metrics" })).is_err());
        let refused = validate(&json!({ "space": "air", "aspect": "usage", "pipeline": "p" }));
        assert!(
            matches!(&refused, Err(OpError::InvalidInput { path, .. }) if path == "/pipeline"),
            "{refused:?}"
        );
    }

    #[test]
    fn drift_keeps_the_findings_of_the_space_asked() {
        let found =
            json!({ "items": [{ "space": "air", "id": "a" }, { "space": "bus", "id": "b" }] });
        assert_eq!(
            of_space(found, "air")["items"],
            json!([{ "space": "air", "id": "a" }])
        );
    }

    use super::super::testkit::{admin, caller, envelope, person, state};
    use super::super::{call, find, Via};
    use axum::response::IntoResponse;

    fn outcome(result: Result<Value, OpError>) -> String {
        match result {
            Ok(answer) => format!(
                "200 {}",
                answer.as_object().map_or(String::new(), |map| map
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","))
            ),
            Err(err) => err.into_response().status().as_u16().to_string(),
        }
    }

    async fn each_aspect(
        caller: &super::super::Caller,
        state: &crate::state::AppState,
    ) -> Vec<String> {
        let op = find(GOVERNANCE).expect("listed");
        let mut got = Vec::new();
        for aspect in [
            "quality",
            "qualityChecks",
            "usage",
            "temporalAudit",
            "surfaceFit",
            "drift",
        ] {
            let mut input = json!({ "space": "air", "aspect": aspect });
            if aspect == "temporalAudit" {
                input["pipeline"] = json!("sensors");
            }
            got.push(format!(
                "{aspect}: {}",
                outcome(call(op, caller, state, "helsinki", input).await)
            ));
        }
        got
    }

    /// Each aspect is the route's own reading of the space, on a Portal with no database or
    /// broker: what such a Portal can say, or the route's reason it cannot.
    #[tokio::test]
    async fn each_aspect_answers_the_routes_reading() {
        let state = state();
        state
            .mirror
            .upsert(envelope("ContextSpace", "air", "helsinki", json!({})));
        let got = each_aspect(&admin(&state), &state).await;
        // usage counts on the broker this Portal has none of; the audit names a pipeline the
        // project does not hold.
        let want = [
            "quality: 200 ",
            "qualityChecks: 200 at,rules,windows",
            "usage: 503",
            "temporalAudit: 404",
            "surfaceFit: 200 dataModel,surfaces",
            "drift: 200 apiVersion,items,kind,metadata",
        ];
        assert_eq!(got, want);
    }

    /// A person who may not read the project learns nothing about its space, aspect by aspect.
    #[tokio::test]
    async fn a_stranger_reads_no_aspect() {
        let state = state();
        state
            .mirror
            .upsert(envelope("ContextSpace", "air", "helsinki", json!({})));
        let got = each_aspect(&caller(person("mallory", &[]), Via::Mcp), &state).await;
        let want = [
            "quality: 404",
            "qualityChecks: 404",
            "usage: 404",
            "temporalAudit: 404",
            "surfaceFit: 404",
            "drift: 404",
        ];
        assert_eq!(got, want);
    }

    /// The body judges its input even when the door's check is skipped.
    #[tokio::test]
    async fn the_body_refuses_a_pipeline_on_another_aspect() {
        let state = state();
        let op = find(GOVERNANCE).expect("listed");
        match (op.run)(
            &admin(&state),
            &state,
            "helsinki",
            json!({ "space": "air", "aspect": "usage", "pipeline": "p" }),
        )
        .await
        {
            Err(OpError::InvalidInput { path, .. }) => assert_eq!(path, "/pipeline"),
            other => panic!("a 422, got {other:?}"),
        }
    }

    /// Drift findings without a list stay as they are.
    #[test]
    fn drift_without_items_is_answered_as_it_came() {
        assert_eq!(
            of_space(json!({ "scanned": false }), "air"),
            json!({ "scanned": false })
        );
    }

    /// Without a pipeline named, the audit reads every Pipeline that writes the space: none
    /// here, so it answers empty rows rather than a 404.
    #[tokio::test]
    async fn an_audit_of_every_pipeline_of_a_space_answers_its_rows() {
        let state = state();
        state
            .mirror
            .upsert(envelope("ContextSpace", "air", "helsinki", json!({})));
        let op = find(GOVERNANCE).expect("listed");
        let audit = call(
            op,
            &admin(&state),
            &state,
            "helsinki",
            json!({ "space": "air", "aspect": "temporalAudit" }),
        )
        .await
        .expect("an audit");
        assert_eq!(audit["rows"], json!([]), "{audit}");
        assert_eq!(audit["unread"], json!([]), "{audit}");
    }
}
