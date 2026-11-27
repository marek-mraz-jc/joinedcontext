#![forbid(unsafe_code)]
//! The dashboard page a person opens from the Portal: `{public}/d/{uid}?orgId={org}`
//! (`crates/portal/src/api/grafana.rs:214`). Every panel's server-rendered table
//! ([`render_panel`]), read with the caller's own rights; no script and no outside origin.

use crate::render::{
    PanelTimeRange, PanelView, dashboard_document, load_dashboard_for_read, render_panel,
};
use crate::state::{DaiyaState, caller};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use daiya_model::{
    Action, Authorizer, CrError, CrResult, Decision, ObjectTarget, OrgId, OrgKey, OrgRow,
    PanelSpec, ReadKind, ReadScope, RequestCtx, SERVER_ADMIN_ACTOR, Subject,
    validation::html_escape,
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::error::Elapsed;

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

/// Actor id prefix of the person a host's [`crate::PageReader`] names: never a principal a
/// binding grants, nor the server administrator.
const PAGE_READER_PREFIX: &str = "page-reader:";

/// Read on the dashboards and datasources of the one organization the host's session lets the
/// person read, for that person, read-only; every other check goes to `inner`. Only the page
/// route installs it, so the person reads saved panels and runs no query of their own.
struct PageReaderAuthorizer {
    org_id: OrgId,
    actor: String,
    inner: std::sync::Arc<dyn Authorizer>,
}

impl Authorizer for PageReaderAuthorizer {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        let reader = action == Action::Read
            && ctx.subject.read_only
            && ctx.subject.actor_id.as_deref() == Some(self.actor.as_str())
            && ctx.org_id == self.org_id
            && object.org_id() == self.org_id
            && ctx.subject.is_member_of(self.org_id);
        if reader {
            Decision::allow("signed-in reader of the organization's dashboards")
        } else {
            self.inner.check(ctx, action, object)
        }
    }

    fn read_scope(&self, ctx: &RequestCtx, kind: ReadKind) -> ReadScope {
        let reader = ctx.subject.read_only
            && ctx.subject.actor_id.as_deref() == Some(self.actor.as_str())
            && ctx.org_id == self.org_id
            && ctx.subject.is_member_of(self.org_id);
        if reader {
            ReadScope::All
        } else {
            self.inner.read_scope(ctx, kind)
        }
    }
}

/// The caller as the host's page reader names them, for a request without a credential:
/// a read-only member of `ctx`'s organization alone, and the state that lets them read it.
fn page_reader(
    state: &DaiyaState,
    headers: &HeaderMap,
    ctx: &RequestCtx,
    org: &OrgRow,
) -> Option<(Subject, DaiyaState)> {
    let reader = state.page_reader.as_ref()?;
    if ctx.subject.actor_id.is_some() || headers.contains_key(header::AUTHORIZATION) {
        return None;
    }
    let person = reader(headers, org.number, &org.name).filter(|p| !p.is_empty())?;
    let actor = format!("{PAGE_READER_PREFIX}{person}");
    let mut subject = Subject::authenticated(actor.clone(), vec![ctx.org_id], Vec::new());
    subject.read_only = true;
    let mut scoped = state.clone();
    scoped.authorizer = std::sync::Arc::new(PageReaderAuthorizer {
        org_id: ctx.org_id,
        actor,
        inner: state.authorizer.clone(),
    });
    Some((subject, scoped))
}

/// How long one dashboard page may take, every panel included.
const PAGE_DEADLINE: Duration = Duration::from_secs(30);

/// How many panels of one page are read at once.
const PAGE_PANELS_AT_ONCE: usize = 8;

/// One panel of the page; a panel past the deadline or in error names only its title.
fn panel_section(title: &str, view: Result<CrResult<PanelView>, Elapsed>) -> String {
    let inner = match view {
        Ok(Ok(view)) => {
            let mut out = format!("<h2>{}</h2>", html_escape(&view.panel_title));
            if let Some(text) = &view.description {
                out.push_str(&format!("<p>{}</p>", html_escape(text)));
            }
            match &view.access_denied_message {
                Some(message) => out.push_str(&format!("<p>{}</p>", html_escape(message))),
                // Escaped by `render_frames_html_table` / `sanitize_markdown_to_html`.
                None => out.push_str(&view.html_table),
            }
            out
        }
        // The cause can name an upstream address: the page says only that it failed.
        Ok(Err(_)) => format!(
            "<h2>{}</h2><p>This panel could not be read.</p>",
            html_escape(title)
        ),
        Err(_) => format!(
            "<h2>{}</h2><p>This panel took too long to read. Reload the page to try again.</p>",
            html_escape(title)
        ),
    };
    format!("<section>{inner}</section>")
}

async fn dashboard_page(
    mut state: DaiyaState,
    headers: HeaderMap,
    uid: String,
    params: PageParams,
) -> Response {
    let (subject, own_org) = match caller(&state, &headers).await {
        Ok(found) => found,
        Err(e) => return refused(&e),
    };
    // The link names the Organization, else it is the caller's; none is never the main one.
    let (org_id, named) = match (params.org_id, own_org) {
        (Some(num), _) => match state.organization(OrgKey::Number(num)).await {
            Ok(Some(org)) => (org.org_id, Some(org)),
            Ok(None) => return refused(&CrError::NotFound("organization".into())),
            Err(e) => return refused(&e),
        },
        (None, Some(id)) => match state.organization(OrgKey::Id(id)).await {
            Ok(org) => (id, org),
            Err(e) => return refused(&e),
        },
        (None, None) => return refused(&CrError::NotFound("organization".into())),
    };
    let mut ctx = state.ctx_in(subject, org_id);
    if let Some(org) = &named {
        // The server administrator is a member of the live organization the link names.
        if ctx.subject.actor_id.as_deref() == Some(SERVER_ADMIN_ACTOR)
            && !ctx.subject.is_member_of(org.org_id)
        {
            ctx.subject.organizations.push(org.org_id);
        }
        if let Some((subject, scoped)) = page_reader(&state, &headers, &ctx, org) {
            ctx.subject = subject;
            state = scoped;
        }
    }
    let record = match load_dashboard_for_read(&state, &ctx, &uid).await {
        Ok(found) => found,
        Err(e) => return refused(&e),
    };
    // A stored document that no longer reads draws the page with no panel and no error text.
    let Ok(doc) = dashboard_document(&record) else {
        return html(StatusCode::OK, &record.title, "");
    };
    let mut ids = Vec::new();
    panel_ids(doc.panels.as_deref().unwrap_or(&[]), &mut ids);
    let range = PanelTimeRange {
        from: params.from.clone(),
        to: params.to.clone(),
        ..PanelTimeRange::default()
    };
    // One deadline for the page and a few panels at a time (PRF-03, PRF-06): a slow source
    // answers its own panel late, never the whole page past the deadline.
    let deadline = tokio::time::Instant::now() + PAGE_DEADLINE;
    let (state, ctx, record_ref, doc) = (&state, &ctx, &record, &doc);
    let sections: Vec<String> = stream::iter(ids)
        .map(|(id, title)| {
            let range = range.clone();
            async move {
                let view = tokio::time::timeout_at(
                    deadline,
                    render_panel(state, ctx, record_ref, doc, id, range),
                )
                .await;
                panel_section(&title, view)
            }
        })
        .buffered(PAGE_PANELS_AT_ONCE)
        .collect()
        .await;
    let body = sections.concat();
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

#[cfg(test)]
mod tests {
    use super::*;
    use daiya_model::{InstallationId, InstanceId, PolicyAuthorizer};

    const ORG: OrgId = OrgId::from_u128(41);
    const OTHER: OrgId = OrgId::from_u128(42);

    fn reader() -> PageReaderAuthorizer {
        PageReaderAuthorizer {
            org_id: ORG,
            actor: format!("{PAGE_READER_PREFIX}anna"),
            inner: std::sync::Arc::new(PolicyAuthorizer::new(Vec::new())),
        }
    }

    fn ctx(actor: &str, org: OrgId, read_only: bool) -> RequestCtx {
        let mut subject = Subject::authenticated(actor, vec![org], Vec::new());
        subject.read_only = read_only;
        RequestCtx::new(
            InstallationId::SINGLE,
            org,
            InstanceId::from_u128(1),
            subject,
        )
    }

    /// A panel the caller may not read names why, a panel that failed names only its title, and
    /// both escape what the dashboard says.
    #[test]
    fn a_refused_or_failed_panel_says_so_and_escapes_its_title() {
        let view = crate::render::PanelView {
            dashboard_uid: "d".into(),
            dashboard_title: "D".into(),
            panel_id: 1,
            panel_title: "<P>".into(),
            panel_type: "timeseries".into(),
            description: None,
            grid_pos: None,
            field_config: serde_json::Value::Null,
            options: serde_json::Value::Null,
            frame_sets: Vec::new(),
            html_table: "<table>".into(),
            access_denied_message: Some("No access to <ds>".into()),
        };
        let shown = panel_section("ignored", Ok(Ok(view)));
        assert_eq!(
            shown,
            "<section><h2>&lt;P&gt;</h2><p>No access to &lt;ds&gt;</p></section>"
        );
        let failed = panel_section("<T>", Ok(Err(CrError::Internal("http://10.0.0.1".into()))));
        assert_eq!(
            failed,
            "<section><h2>&lt;T&gt;</h2><p>This panel could not be read.</p></section>"
        );
    }

    #[tokio::test]
    async fn a_panel_past_its_deadline_says_reload() {
        let late = tokio::time::timeout(
            std::time::Duration::ZERO,
            std::future::pending::<CrResult<crate::render::PanelView>>(),
        )
        .await;
        assert_eq!(
            panel_section("<T>", late),
            "<section><h2>&lt;T&gt;</h2><p>This panel took too long to read. Reload the page to try again.</p></section>"
        );
    }

    /// The host's reader reads every dashboard of its one organization, read-only; anything
    /// else (a write, another organization, another person, a subject that may write) is
    /// the inner authorizer's, which grants this reader nothing.
    #[test]
    fn the_page_reader_reads_its_organization_alone_and_nothing_else() {
        let auth = reader();
        let anna = format!("{PAGE_READER_PREFIX}anna");
        let board = |org_id| ObjectTarget::Dashboard {
            org_id,
            dashboard_uid: "b".into(),
            folder_uid: None,
        };
        let own = ctx(&anna, ORG, true);
        assert!(auth.check(&own, Action::Read, &board(ORG)).allowed);
        assert!(matches!(
            auth.read_scope(&own, ReadKind::Dashboard),
            ReadScope::All
        ));
        assert!(!auth.check(&own, Action::Delete, &board(ORG)).allowed);
        assert!(!auth.check(&own, Action::Read, &board(OTHER)).allowed);
        for other in [
            ctx(&anna, ORG, false),
            ctx(&format!("{PAGE_READER_PREFIX}bob"), ORG, true),
            ctx(&anna, OTHER, true),
        ] {
            assert!(
                !auth
                    .check(&other, Action::Read, &board(other.org_id))
                    .allowed
            );
            assert!(matches!(
                auth.read_scope(&other, ReadKind::Dashboard),
                ReadScope::Nothing
            ));
        }
    }
}
