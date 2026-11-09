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

/// The page's only style, admitted by its hash: the page carries no script and loads nothing.
const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#f4f5f5;--card:#fff;--fg:#24292e;--muted:#5b6168;--line:#d8dbdf}\
@media (prefers-color-scheme:dark){:root{--bg:#111217;--card:#181b1f;--fg:#ccccdc;--muted:#9fa7b3;--line:#2c3235}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 Inter,system-ui,-apple-system,'Segoe UI',sans-serif}\
main{max-width:1200px;margin:0 auto;padding:24px 16px}\
h1{font-size:22px;font-weight:500;margin:0 0 16px}\
h2{font-size:14px;font-weight:500;margin:0 0 8px}\
section{background:var(--card);border:1px solid var(--line);border-radius:6px;padding:12px 16px;margin:0 0 16px;overflow-x:auto}\
p{color:var(--muted);margin:0 0 8px}\
table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}\
th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line);white-space:nowrap}\
th{font-weight:500;color:var(--muted)}\
a{color:#3d71d9}";

/// The Content-Security-Policy: nothing from anywhere, except [`STYLE`] by its SHA-256.
static CSP: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use base64::Engine as _;
    use sha2::Digest as _;
    let hash = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(STYLE));
    format!("default-src 'none'; style-src 'sha256-{hash}'; frame-ancestors 'self'")
});

fn html(status: StatusCode, title: &str, body: &str) -> Response {
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style></head><body><main><h1>{title}</h1>{body}\
         </main></body></html>",
        title = html_escape(title),
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::CONTENT_SECURITY_POLICY, CSP.as_str()),
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
    html(
        status,
        "Dashboard not found",
        "<section><p>This dashboard does not exist in this organization, or you may not open it. \
         Check the address, or open the dashboard again from its project in the Portal.</p>\
         <p><a href=\"/\">Go to the start page</a></p></section>",
    )
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
