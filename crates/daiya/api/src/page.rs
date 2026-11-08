#![forbid(unsafe_code)]
//! The dashboard page a person opens from the Portal: `{public}/d/{uid}?orgId={org}`
//! (`crates/portal/src/api/grafana.rs:214`). Every panel's server-rendered table
//! ([`panel_view`]), read with the caller's own rights; no script and no outside origin.

use crate::render::{PanelTimeRange, panel_view};
use crate::state::{DaiyaState, resolve_request_ctx};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use daiya_model::{
    Action, CrError, DashboardDocument, ObjectTarget, PanelSpec, validation::html_escape,
};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct PageParams {
    #[serde(rename = "orgId")]
    org_id: Option<i64>,
    from: Option<String>,
    to: Option<String>,
}

/// The id and title of each drawable panel, rows unfolded, in dashboard order.
fn panel_ids(panels: &[PanelSpec], out: &mut Vec<(u32, String)>) {
    for p in panels {
        if p.panel_type != "row"
            && let Some(id) = p.id
        {
            let title = p.title.clone().unwrap_or_else(|| format!("Panel {id}"));
            out.push((id, title));
        }
        if let Some(nested) = &p.panels {
            panel_ids(nested, out);
        }
    }
}

fn html(status: StatusCode, title: &str, body: &str) -> Response {
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title></head><body><main><h1>{title}</h1>{body}</main></body></html>",
        title = html_escape(title),
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; frame-ancestors 'self'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        page,
    )
        .into_response()
}

/// A refusal as a page: a hidden denial and a missing dashboard read the same (404).
fn refused(err: &CrError) -> Response {
    let status = StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::NOT_FOUND);
    html(status, "Dashboard not found", "")
}

async fn dashboard_page(
    state: DaiyaState,
    headers: HeaderMap,
    uid: String,
    params: PageParams,
) -> Response {
    let mut ctx = resolve_request_ctx(&state, &headers);
    if let Some(num) = params.org_id {
        let Some(org) = state
            .org_registry
            .read()
            .by_num
            .get(&num)
            .map(|(_, org)| *org)
        else {
            return refused(&CrError::NotFound("organization".into()));
        };
        ctx.org_id = org;
    }
    let record = match state.store.get_dashboard(ctx.org_id, &uid).await {
        Ok(Some(record)) => record,
        Ok(None) => return refused(&CrError::NotFound("dashboard".into())),
        Err(e) => return refused(&e),
    };
    let target = ObjectTarget::Dashboard {
        org_id: ctx.org_id,
        dashboard_uid: record.uid.clone(),
        folder_uid: record.folder_uid.clone(),
    };
    if let Err(e) = state
        .authorizer
        .check(&ctx, Action::Read, &target)
        .into_result("Dashboard not found")
    {
        return refused(&e);
    }
    let mut ids = Vec::new();
    if let Ok(doc) = serde_json::from_value::<DashboardDocument>(record.data.clone()) {
        panel_ids(doc.panels.as_deref().unwrap_or(&[]), &mut ids);
    }
    let mut body = String::new();
    for (id, title) in ids {
        let range = PanelTimeRange {
            from: params.from.clone(),
            to: params.to.clone(),
            ..PanelTimeRange::default()
        };
        body.push_str("<section>");
        match panel_view(&state, &ctx.subject, ctx.org_id, &uid, id, range).await {
            Ok(view) => {
                body.push_str(&format!("<h2>{}</h2>", html_escape(&view.panel_title)));
                if let Some(text) = &view.description {
                    body.push_str(&format!("<p>{}</p>", html_escape(text)));
                }
                match &view.access_denied_message {
                    Some(message) => body.push_str(&format!("<p>{}</p>", html_escape(message))),
                    // Escaped by `render_frames_html_table` / `sanitize_markdown_to_html`.
                    None => body.push_str(&view.html_table),
                }
            }
            // The cause can name an upstream address: the page says only that it failed.
            Err(_) => body.push_str(&format!(
                "<h2>{}</h2><p>This panel could not be read.</p>",
                html_escape(&title)
            )),
        }
        body.push_str("</section>");
    }
    html(StatusCode::OK, &record.title, &body)
}

async fn page(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(uid): Path<String>,
    Query(params): Query<PageParams>,
) -> Response {
    dashboard_page(state, headers, uid, params).await
}

async fn page_with_slug(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path((uid, _slug)): Path<(String, String)>,
    Query(params): Query<PageParams>,
) -> Response {
    dashboard_page(state, headers, uid, params).await
}

/// `GET /d/{uid}` and `GET /d/{uid}/{slug}`, as Grafana links a dashboard.
pub fn page_router() -> Router<DaiyaState> {
    Router::new()
        .route("/d/{uid}", get(page))
        .route("/d/{uid}/{slug}", get(page_with_slug))
}
