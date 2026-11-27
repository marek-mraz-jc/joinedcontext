//! The audit trail of a project's configuration as a file (T-5114): the closed changes
//! `GET …/changes/history` lists, under the same filter and read rule, as CSV.
//!
//! - `GET /api/v1/projects/{project}/changes/history/export`

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Router;

use super::changes::{change_history_readable, ChangeProposal, HistoryQuery};
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// History pages one export reads, 20 closed merge requests each.
// ponytail: 200 closed merge requests per file; a decided-change table lifts the cap.
const EXPORT_PAGES: u32 = 10;
/// The page an export stopped before, while the forge holds older closed changes.
pub const NEXT_PAGE: &str = "jc-next-page";
const COLUMNS: &str =
    "closedAt,change,outcome,kind,name,operation,proposedBy,decidedBy,reason,mergeRequest";

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes/history/export",
    summary = "Export Closed Changes",
    description = "The closed changes the history lists, under its filter and read rule, as CSV, \
                   newest first: when, what, the outcome, who proposed and who decided, and the \
                   rejection reason. One file covers up to 200 closed merge requests from `page`; \
                   the `JC-Next-Page` header names the page that continues it.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        HistoryQuery,
    ),
    responses(
        (status = 200, description = "text/csv, one row per closed change", content_type = "text/csv", body = String),
        (status = 400, description = "An unknown parameter, page 0 or a day that is not YYYY-MM-DD", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn export_change_history(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    query: Result<Query<HistoryQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(mut query) = query.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let first = query.page.unwrap_or(1);
    let mut items = Vec::new();
    let mut next = Some(first);
    while let Some(page) = next.filter(|page| *page < first.saturating_add(EXPORT_PAGES)) {
        query.page = Some(page);
        let list = change_history_readable(&state, &user.0.identity, &project, &query).await?;
        items.extend(list.items);
        next = list.next;
    }
    Ok(csv_file(&project, &csv(&items), next))
}

/// `csv` as a download named after `project` (a DNS-1123 name), never kept by a cache.
fn csv_file(project: &str, csv: &str, next: Option<u32>) -> Response {
    let file = format!("attachment; filename=\"{project}-changes.csv\"");
    let mut answer = (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, file),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        csv.to_owned(),
    )
        .into_response();
    if let Some(next) = next {
        answer
            .headers_mut()
            .insert(NEXT_PAGE, HeaderValue::from(next));
    }
    answer
}

/// The rows of `items`, a header line first. Names only: the proposer's e-mail stays out of a
/// file that leaves the Portal.
fn csv(items: &[ChangeProposal]) -> String {
    let mut out = format!("{COLUMNS}\r\n");
    for change in items {
        let param = |key: &str| {
            change
                .summary
                .params
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let decision = change.decision.as_ref();
        let row = [
            decision
                .and_then(|d| d.at.clone())
                .unwrap_or_else(|| change.created_at.clone()),
            change.metadata.name.clone(),
            format!("{:?}", change.status.phase),
            param("kind"),
            param("name"),
            change
                .summary
                .key
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_owned(),
            change.author.name.clone(),
            decision.map(|d| d.by.clone()).unwrap_or_default(),
            decision.and_then(|d| d.reason.clone()).unwrap_or_default(),
            change.status.merge_request.clone().unwrap_or_default(),
        ];
        let cells: Vec<String> = row.iter().map(|value| cell(value)).collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

/// One RFC 4180 field. A value a spreadsheet would run as a formula starts with `'` (OWASP CSV
/// injection): a rejection reason is free text a proposer's colleague typed. The cost export
/// (`crate::costs`) writes its cells with it too.
pub(crate) fn cell(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/changes/history/export",
        axum::routing::get(export_change_history),
    )
}

#[cfg(test)]
mod tests {
    use super::{cell, csv};
    use crate::api::changes::{ChangeAuthor, ChangeDecision, ChangeProposal, ChangeSummary};
    use crate::change::{ChangeMeta, ChangePhase, ChangeStatus, Lane, PlanSummary};

    fn closed(name: &str, decision: Option<ChangeDecision>) -> ChangeProposal {
        ChangeProposal {
            api_version: "joinedcontext.com/v1alpha1".into(),
            kind: "ChangeProposal".into(),
            metadata: ChangeMeta::new(name, "helsinki"),
            status: ChangeStatus::new(Lane::Green, ChangePhase::Merged, PlanSummary::default()),
            summary: ChangeSummary {
                key: "change.update".into(),
                params: serde_json::Map::new(),
            },
            author: ChangeAuthor {
                name: "Jana".into(),
                email: Some("jana@hel.fi".into()),
            },
            created_at: "2026-10-01T09:00:00Z".into(),
            plan_fields: None,
            files: None,
            file_count: None,
            workspace: None,
            waits_on: Vec::new(),
            decision,
            ci: None,
            space: None,
        }
    }

    /// T-7917, T-5114: a change closed in the forge itself has no recorded decision, and one whose
    /// decision carries no time, take the day the change was created as their `closedAt`; the
    /// e-mail of the proposer is never a column.
    #[test]
    fn a_change_with_no_recorded_time_is_dated_by_its_creation() {
        let timed = ChangeDecision {
            by: "ada".into(),
            via: None,
            at: Some("2026-10-02T10:00:00Z".into()),
            reason: None,
        };
        let untimed = ChangeDecision {
            at: None,
            ..timed.clone()
        };
        let rows = csv(&[
            closed("chg-00000001", None),
            closed("chg-00000002", Some(untimed)),
            closed("chg-00000003", Some(timed)),
        ]);
        let lines: Vec<&str> = rows.lines().collect();
        assert_eq!(lines.len(), 4, "{rows}");
        assert!(
            lines[1].starts_with("2026-10-01T09:00:00Z,chg-00000001,Merged,"),
            "{rows}"
        );
        assert!(
            lines[2].starts_with("2026-10-01T09:00:00Z,chg-00000002,Merged,"),
            "{rows}"
        );
        assert!(
            lines[3].starts_with("2026-10-02T10:00:00Z,chg-00000003,Merged,"),
            "{rows}"
        );
        assert!(!rows.contains("jana@hel.fi"), "{rows}");
    }

    #[test]
    fn a_cell_is_quoted_when_it_must_be_and_a_formula_is_defused() {
        assert_eq!(cell("public-air"), "public-air");
        assert_eq!(cell("a, b"), "\"a, b\"");
        assert_eq!(cell("say \"no\""), "\"say \"\"no\"\"\"");
        assert_eq!(cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(cell("-1"), "'-1");
        assert_eq!(cell("line\nbreak"), "\"line\nbreak\"");
    }
}
