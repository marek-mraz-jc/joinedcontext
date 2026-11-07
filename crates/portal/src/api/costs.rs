//! `GET /api/v1/organization/costs` and its CSV: what a month cost each project (T-5126). The
//! split rule is `crate::costs`'s; this file gathers the measures: each space's bytes from the
//! broker that holds it (API/01 §29) and each AssistantDeployment's priced turns from the
//! assistant (API/05 §3.1). Organization administrators only: the page names every project.

use std::collections::BTreeMap;
use std::time::Instant;

use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use super::knowledge::{admitted, ask_with, LIST_TIME};
use crate::auth::session::CurrentUser;
use crate::costs::{AiSpend, AssistantMeasure, CostPrices, SpaceMeasure, Split};
use crate::error::ApiError;
use crate::state::AppState;

/// Broker and assistant reads in flight at once: each is cached or bounded by its own timeout.
const AT_ONCE: usize = 8;

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct CostQuery {
    /// The calendar month, e.g. `2026-09`; the last whole month when absent.
    pub month: Option<String>,
}

/// A month of the installation split per project, in the price table's currency.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Costs {
    pub month: String,
    pub currency: String,
    /// When the bytes were measured: storage is read now, not averaged over the month.
    #[schema(value_type = String, format = DateTime)]
    pub measured_at: DateTime<Utc>,
    pub prices: CostPrices,
    #[serde(flatten)]
    pub split: Split,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/costs",
    summary = "Read Costs per Project",
    description = "One calendar month split per project (T-5126): the nodes and fixed items by the project's share of the bytes its spaces store, its bytes at the blended volume price, and each AssistantDeployment's priced turns at the table's rate. What no project carries is the platform's; the projects and the platform add up to the operator's price table. Organization administrators only.",
    tag = "system",
    params(CostQuery),
    responses(
        (status = 200, description = "The month split", body = Costs),
        (status = 400, description = "month is no started calendar month such as 2026-09", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
        (status = 503, description = "No price table is configured", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_costs(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<CostQuery>,
) -> Result<Json<Costs>, ApiError> {
    Ok(Json(costs(&state, &user, query).await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/costs/export",
    summary = "Export Costs per Project",
    description = "The month of `GET /api/v1/organization/costs` as CSV: one line per node share, space and assistant, the platform's lines last (T-5126). Organization administrators only.",
    tag = "system",
    params(CostQuery),
    responses(
        (status = 200, description = "text/csv", content_type = "text/csv", body = String),
        (status = 400, description = "month is no started calendar month such as 2026-09", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
        (status = 503, description = "No price table is configured", body = crate::error::ProblemDetails),
    )
)]
pub async fn export_costs(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<CostQuery>,
) -> Result<Response, ApiError> {
    let costs = costs(&state, &user, query).await?;
    let csv = crate::costs::csv(&costs.month, &costs.currency, &costs.split);
    // The month is checked to be `YYYY-MM`, so the name needs no escaping.
    let file = format!("attachment; filename=\"costs-{}.csv\"", costs.month);
    let headers = [
        (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
        (header::CONTENT_DISPOSITION, file),
        (header::CACHE_CONTROL, "no-store".to_owned()),
    ];
    Ok((headers, csv).into_response())
}

async fn costs(state: &AppState, user: &CurrentUser, query: CostQuery) -> Result<Costs, ApiError> {
    let identity = &user.0.identity;
    crate::permissions::require_organization_admin(state, identity, "the Costs page")?;
    let now = Utc::now();
    let month = query.month.unwrap_or_else(|| crate::costs::last_month(now));
    let (from, to) = crate::costs::month(&month, now).map_err(ApiError::BadRequest)?;
    let prices = state.config.cost_prices.clone().ok_or_else(|| {
        ApiError::Unavailable(
            "no price table is set: the operator sets portal.costPrices in the deployment values \
             (JC_PORTAL_COST_PRICES_FILE) with each node, volume and fixed item"
                .into(),
        )
    })?;
    // The assistant reads count toward the caller's rate on the stored turns, as one call.
    admitted(&identity.subject, Instant::now())?;
    let mut measured: BTreeMap<String, (Vec<SpaceMeasure>, Vec<AssistantMeasure>)> =
        BTreeMap::new();
    let of_kind = |kind: &str| {
        let mut found: Vec<(String, String)> = state
            .mirror
            .matching(|env| env.kind == kind)
            .into_iter()
            .filter_map(|env| Some((env.metadata.namespace?, env.metadata.name)))
            .filter(|(project, _)| project != crate::permissions::ORG_NAMESPACE)
            .collect();
        found.sort();
        found
    };
    let spaces = futures_util::stream::iter(of_kind("ContextSpace"))
        .map(|(project, space)| async move {
            let broker = crate::spaces::broker(&state.mirror, &project, &space);
            let tenant = crate::spaces::segment(&state.mirror, &project, &space);
            let bytes = match state.space_usage.usage(&broker, &tenant).await {
                Ok(usage) => Ok(usage.bytes.map_or(0, |b| b.entities + b.attr_instances)),
                Err(why) => Err(why),
            };
            (project, SpaceMeasure { space, bytes })
        })
        .buffer_unordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await;
    let (from, to) = (from.to_rfc3339(), to.to_rfc3339());
    let assistants = futures_util::stream::iter(of_kind("AssistantDeployment"))
        .map(|(project, assistant)| {
            let (from, to) = (from.clone(), to.clone());
            async move {
                let spend = spend(state, &project, &assistant, &from, &to).await;
                (project, AssistantMeasure { assistant, spend })
            }
        })
        .buffer_unordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await;
    for (project, space) in spaces {
        measured.entry(project).or_default().0.push(space);
    }
    for (project, assistant) in assistants {
        measured.entry(project).or_default().1.push(assistant);
    }
    for (spaces, assistants) in measured.values_mut() {
        spaces.sort_by(|a, b| a.space.cmp(&b.space));
        assistants.sort_by(|a, b| a.assistant.cmp(&b.assistant));
    }
    Ok(Costs {
        split: crate::costs::split(&prices, measured),
        currency: prices.currency.clone(),
        prices,
        month,
        measured_at: now,
    })
}

/// One assistant's priced turns of `[from, to)`, as the assistant sums them.
async fn spend(
    state: &AppState,
    project: &str,
    assistant: &str,
    from: &str,
    to: &str,
) -> Result<AiSpend, String> {
    let path = format!("{project}/knowledge/conversations/usage");
    let range = [("deployment", assistant), ("from", from), ("to", to)];
    let query = |r: reqwest::RequestBuilder| r.query(&range);
    let (status, body) = ask_with(state, reqwest::Method::GET, &path, LIST_TIME, query)
        .await
        .map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("the assistant answered {status}"));
    }
    Ok(ai_spend(&body))
}

/// The summary's totals: an unpriced turn is counted apart, never as $0 (API/05 §3.1).
fn ai_spend(body: &Value) -> AiSpend {
    AiSpend {
        cost_usd: body["costUsd"]["total"].as_f64().unwrap_or(0.0),
        questions: body["questions"].as_u64().unwrap_or(0),
        uncosted_questions: body["uncostedQuestions"].as_u64().unwrap_or(0),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/costs", get(get_costs))
        .route("/organization/costs/export", get(export_costs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_totals_are_read_and_a_missing_one_is_zero() {
        let body = serde_json::json!({
            "questions": 12, "uncostedQuestions": 1, "costUsd": {"total": 0.42, "mean": 0.04}
        });
        let spend = ai_spend(&body);
        assert_eq!(
            (spend.cost_usd, spend.questions, spend.uncosted_questions),
            (0.42, 12, 1)
        );
        assert_eq!(
            ai_spend(&serde_json::json!({"costUsd": {"total": null}})),
            AiSpend::default()
        );
    }
}
