#![forbid(unsafe_code)]
//! `shako-api`: Git smart HTTP, Gitea 1.27.3 REST compatibility subset (`COMPAT.md`),
//! native REST (`/api/v1/projects/{project}/repositories/...`), OData 4.01 service (`/odata/`),
//! and transactional outbox / webhook dispatcher.

pub mod compat;
pub mod mirror;
pub mod odata;
pub mod outbox;
pub mod service;
pub mod smart_http;
pub mod state;
pub mod work;

pub use compat::{compat_and_native_rest_router, pr_to_gitea_json};
pub use mirror::drain_mirrors;
pub use odata::{
    evaluate_allowed_query, exec_odata_operation, find_visible_repo_by_key, odata_router,
};
pub use outbox::{
    build_pr_webhook_payload, build_push_webhook_payload, compute_gitea_signature,
    deliver_webhook_now, dispatch_logged, dispatch_pending_outbox, emit_and_dispatch_event,
    record_event, verify_gitea_signature,
};
pub use smart_http::smart_http_router;
pub use state::{
    InProcessEventSubscriber, ShakoState, authenticate_request, gitea_error_response,
    odata_error_response, problem_error_response, read_body_bounded, repo_to_gitea_json,
    resolve_and_authorize_repo, stream_file_in_64k_chunks, strip_dot_git,
};

use axum::Router;

/// Builds the combined `shako-api` router (Smart HTTP + Gitea compat REST + Native REST + OData 4.01).
pub fn api_router() -> Router<ShakoState> {
    Router::new()
        .merge(smart_http_router())
        .merge(compat_and_native_rest_router())
        .merge(odata_router())
}
