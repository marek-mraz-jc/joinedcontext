//! `If-Match` on the manifest writes (API/13 section 3), shared by the update, patch and delete
//! doors; a module of its own so those doors and the listing in `resources` do not call each other.

use axum::http::{header, HeaderMap};

use crate::error::ApiError;
use crate::resource::by_plural;
use crate::state::AppState;

/// `If-Match` of a `PUT`, `PATCH` or `DELETE` (API/13 section 3): a stale value is `412` with
/// the current `ETag`. Checked only on a manifest the caller may read, so the answer never tells
/// a caller that an unreadable object exists; the write's own door answers that case.
pub async fn check_if_match(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    plural: &str,
    name: &str,
    workspace: Option<&str>,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let Some(sent) = headers.get(header::IF_MATCH) else {
        return Ok(());
    };
    let sent = sent
        .to_str()
        .map_err(|_| ApiError::BadRequest("If-Match must be a quoted ETag or *".into()))?;
    let Some(info) = by_plural(plural) else {
        return Ok(());
    };
    let effective = crate::permissions::for_request(state, identity, project);
    let current = match workspace {
        Some(workspace) => crate::ops::workspaces::mirror_of(state, identity, workspace, project)
            .await?
            .get(project, info.kind, name),
        None => state.mirror.get(project, info.kind, name),
    };
    let Some(current) = current.filter(|env| effective.may_read_envelope(info.kind, env)) else {
        return Ok(());
    };
    let etag = current.etag();
    if sent.trim() == "*" || sent.split(',').any(|value| value.trim() == etag) {
        return Ok(());
    }
    Err(ApiError::PreconditionFailed {
        detail: format!(
            "{} '{name}' changed since it was read: read it again and send its ETag {etag} in If-Match",
            info.kind
        ),
        etag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn if_match_is_judged_only_where_the_caller_reads_the_manifest() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let identity = crate::auth::session::Identity {
            client: None,
            subject: "f:1:ana".into(),
            username: "ana".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        };
        let mut headers = HeaderMap::new();
        let call = |headers: HeaderMap, plural: &'static str| {
            let (state, identity) = (state.clone(), identity.clone());
            async move {
                check_if_match(&state, &identity, "ovzdusie", plural, "air", None, &headers).await
            }
        };
        assert!(
            call(headers.clone(), "pipelines").await.is_ok(),
            "no If-Match"
        );
        headers.insert(header::IF_MATCH, "\"stale\"".parse().expect("header"));
        assert!(
            call(headers.clone(), "pipelines").await.is_ok(),
            "nothing there to compare"
        );
        assert!(
            call(headers.clone(), "nosuchplural").await.is_ok(),
            "the door answers 404"
        );
        headers.insert(
            header::IF_MATCH,
            axum::http::HeaderValue::from_bytes(b"\xff").expect("opaque bytes"),
        );
        assert!(matches!(
            call(headers, "pipelines").await,
            Err(ApiError::BadRequest(_))
        ));
    }
}
