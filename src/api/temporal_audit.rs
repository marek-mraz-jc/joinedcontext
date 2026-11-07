//! The temporal audit of a space (T-4844): for every pipeline the caller may read that writes the
//! space, how each attribute it maps becomes history rows, with a verdict (crate::temporal_audit).

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;
use crate::store::ListOptions;
use crate::temporal_audit::{AuditRow, Input, Verdict};

/// Forge reads of `bento.yaml` in flight at once.
const READS_AT_ONCE: usize = 8;

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct AuditQuery {
    /// Only this pipeline.
    pub pipeline: Option<String>,
}

/// The audit of one space.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemporalAudit {
    /// One row per pipeline, entity type and attribute, by pipeline, type and attribute.
    pub rows: Vec<AuditRow>,
    /// The pipelines whose `bento.yaml` the forge did not answer: their rows may be missing.
    pub unread: Vec<String>,
    /// The rows of each verdict.
    pub verdicts: BTreeMap<String, usize>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/temporal-audit",
    summary = "Audit How Pipelines Create History",
    description = "For every pipeline the caller may read that writes the space: each entity type and attribute its mapping writes, the record mode the space's retention gives it (DM-108), where its observedAt comes from (mapped, constant, or stamped on receipt, DM-107), its unitCode and datasetId, the type's history and the pipeline's lifetime, and a verdict: ok, loses-history, unbounded-growth, duplicates-rows or stamped-on-receive (T-4844). Names only, never a value of the data.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        AuditQuery,
    ),
    responses(
        (status = 200, description = "The rows and the count of each verdict", body = TemporalAudit),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such space the caller may read, or no such pipeline the caller may read", body = ProblemDetails),
    )
)]
pub async fn temporal_audit(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<TemporalAudit>, ApiError> {
    crate::api::data_views::readable_space(&state, &user.0.identity, &project, &space)?;
    let pipelines = writers(&state, &user, &project, &space, query.pipeline.as_deref())?;
    Ok(Json(audit(&state, &project, &space, pipelines).await))
}

/// The pipelines the caller may read that write `space`; one named pipeline that is not one of
/// them is a 404.
fn writers(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
    only: Option<&str>,
) -> Result<Vec<crate::resource::ResourceEnvelope>, ApiError> {
    let names: Vec<String> = match only {
        Some(name) => vec![name.to_owned()],
        None => state
            .mirror
            .list(project, "Pipeline", &ListOptions::default())
            .items
            .into_iter()
            .map(|p| p.metadata.name)
            .collect(),
    };
    let mut out = Vec::new();
    for name in names {
        let readable = crate::api::pipelines::readable_pipeline(state, user, project, &name);
        let pipeline = match (readable, only) {
            (Ok(p), _) => p,
            (Err(e), Some(_)) => return Err(e),
            (Err(_), None) => continue,
        };
        let writes = crate::resource::pipeline_targets(&pipeline.spec)
            .iter()
            .any(|target| {
                let endpoint = target.rsplit(':').next().unwrap_or(target);
                crate::pipeline_expiry::space_of_endpoint(&state.mirror, project, endpoint)
                    .as_deref()
                    == Some(space)
            });
        match (writes, only) {
            (true, _) => out.push(pipeline),
            (false, Some(name)) => {
                return Err(ApiError::NotFound(format!(
                    "pipeline '{name}' does not write context space '{space}'"
                )))
            }
            (false, None) => {}
        }
    }
    Ok(out)
}

/// The one entity type a pipeline's outputs name, if they name exactly one.
fn default_type(spec: &Value) -> Option<String> {
    let mut types: Vec<&str> = spec
        .pointer("/output/type")
        .and_then(Value::as_str)
        .into_iter()
        .collect();
    for output in spec
        .get("outputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        types.extend(output.get("type").and_then(Value::as_str));
    }
    types.dedup();
    match types.as_slice() {
        [one] => Some((*one).to_owned()),
        _ => None,
    }
}

async fn audit(
    state: &AppState,
    project: &str,
    space: &str,
    pipelines: Vec<crate::resource::ResourceEnvelope>,
) -> TemporalAudit {
    use futures_util::stream::{self, StreamExt};
    let retention: Vec<jc_core::kinds::RetentionRule> = state
        .mirror
        .get(project, "ContextSpace", space)
        .and_then(|s| s.spec.get("retention").cloned())
        .and_then(|r| serde_json::from_value(r).ok())
        .unwrap_or_default();
    // The rules the broker reads: the space's with every pipeline's history plan folded in (PL-108).
    let planned = crate::pipeline_history::other_plans(&state.mirror, project, space, "");
    let retention = crate::pipeline_history::fold(&retention, &planned);
    let model = state.model_schemas.get(project, space);
    let unit = |t: &str, a: &str| {
        let class = model.as_ref()?.attributes(t)?;
        let unit = class.attributes.into_iter().find(|x| x.name == a)?.unit?;
        unit.code.or(unit.ucum)
    };
    let read: Vec<(crate::resource::ResourceEnvelope, Result<String, ApiError>)> =
        stream::iter(pipelines)
            .map(|p| async move {
                let text = crate::api::pipelines::read_bento(state, project, &p).await;
                (p, text)
            })
            .buffered(READS_AT_ONCE)
            .collect()
            .await;
    let mut rows = Vec::new();
    let mut unread = Vec::new();
    for (pipeline, bento) in read {
        let name = pipeline.metadata.name.as_str();
        let bento = match bento {
            Ok(text) => Some(text),
            Err(ApiError::NotFound(_)) => None,
            Err(err) => {
                tracing::warn!(project, pipeline = name, error = %err, "temporal audit: bento.yaml unread");
                unread.push(name.to_owned());
                None
            }
        };
        let default_type = default_type(&pipeline.spec);
        rows.extend(crate::temporal_audit::rows(&Input {
            pipeline: name,
            spec: &pipeline.spec,
            bento: bento.as_deref(),
            default_type: default_type.as_deref(),
            retention: &retention,
            unit: &unit,
        }));
    }
    let mut verdicts = BTreeMap::new();
    for verdict in [
        Verdict::Ok,
        Verdict::LosesHistory,
        Verdict::UnboundedGrowth,
        Verdict::DuplicatesRows,
        Verdict::StampedOnReceive,
    ] {
        let name = serde_json::to_value(verdict)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        verdicts.insert(name, rows.iter().filter(|r| r.verdict == verdict).count());
    }
    TemporalAudit {
        rows,
        unread,
        verdicts,
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/spaces/{space}/temporal-audit",
        get(temporal_audit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_type_is_the_one_type_the_outputs_name() {
        assert_eq!(
            default_type(&serde_json::json!({"output": {"type": "Bridge"}})).as_deref(),
            Some("Bridge")
        );
        let two = serde_json::json!({"outputs": [{"type": "A"}, {"type": "B"}]});
        assert_eq!(default_type(&two), None);
        let same = serde_json::json!({"outputs": [{"type": "A"}, {"type": "A"}]});
        assert_eq!(default_type(&same).as_deref(), Some("A"));
        assert_eq!(default_type(&serde_json::json!({})), None);
    }
}
