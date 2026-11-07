#![forbid(unsafe_code)]
//! `controlroom-api`:
//! - Native OData 4.01 service at `/odata/v4` with all 8 entity sets (`odata-table`) and all
//!   bound/unbound actions and functions of R10a (`$metadata`, `$batch`, `$filter`, `$select`,
//!   `$orderby`, `$top`, `$skip`, `$count`, `$allowed`).
//! - In-process `panel_view` function for the Portal and dataroom catalogue (R6, R6b).
//! - Server-rendered HTML first-page data table for no-JS and accessibility (R6, R6b, §9.2).
//! - Public dashboards with 256-bit hashed tokens, strict query/variable lock-down, range/interval
//!   caps, per-IP rate limiting, and metadata sanitization (R7).
//! - Thin Grafana HTTP API compatibility adapters for `provision.sh` and `src/grafana.rs` (R2).

pub mod compat;
pub mod odata;
pub mod public;
pub mod render;
pub mod state;

pub use public::{
    PublicDashboardCreateReq, PublicDashboardQueryReq, PublicDashboardViewDto, PublicRateLimiter,
    create_or_update_public_dashboard, execute_public_dashboard_query, generate_256bit_token,
    rotate_public_dashboard_token, view_public_dashboard,
};
pub use render::{
    PanelTimeRange, PanelView, execute_targets_and_expressions, panel_view,
    render_frames_html_table,
};
pub use state::{ControlroomState, SubjectResolver, TokenBinding, resolve_request_ctx};

use axum::Router;
use jc_router::Mount;

/// Builds the complete `controlroom-api` router (native OData 4.01 + public dashboards + Grafana compat).
pub fn api_router(state: ControlroomState, _mount: Mount) -> Router {
    Router::new()
        .merge(odata::odata_router())
        .merge(compat::compat_router())
        .merge(public::public_router())
        .with_state(state)
}
