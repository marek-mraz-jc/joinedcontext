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
        match validate(&json!({ "space": "air", "aspect": "usage", "pipeline": "p" })) {
            Err(OpError::InvalidInput { path, .. }) => assert_eq!(path, "/pipeline"),
            other => panic!("{other:?}"),
        }
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
}
