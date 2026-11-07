//! Who may call the Portal's internal listener (PF-46, AG-52; docs Architecture/13 §6).
//!
//! The listener is not on the public URL scheme and a NetworkPolicy admits one workload to its
//! port. That is the second control, never the only one: a pod that reaches the port through a
//! policy mistake must still present the identity the route belongs to. CLAUDE.md's rule is the
//! whole of this module — "every workload via a `ServiceAccount` client with audience-bound
//! tokens; no static keys between cluster services".
//!
//! One check, one place, and a client per route (T-2271): the gateway reads the preview list, the
//! credential proxy answers the run callbacks, the project's pipeline runner posts a test capture.
//! A token for one of those opens that route and no other, because the check compares `azp` and not
//! only the audience — the audience says "this listener", the client says "this door".

use axum::http::{header, HeaderMap};

use crate::error::ApiError;
use crate::state::AppState;

/// What a token for this listener must be issued for. Not the Portal's API audience: a token a
/// person or an app holds for `/api/v1` must not open an internal route as well.
pub const INTERNAL_AUDIENCE: &str = "portal-internal";

/// The bearer of this request, without its scheme.
fn presented(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.trim().is_empty())
        .ok_or(ApiError::Unauthorized)
}

/// Verifies that this call carries the ServiceAccount token of the workload this route belongs to:
/// signed by the realm, issued for [`INTERNAL_AUDIENCE`], and obtained by the client the
/// configuration names for that route.
///
/// Fail closed twice over: a Portal with no realm and a Portal that was not told which client owns
/// the route both refuse every call, rather than answering because the network rule let it in.
pub async fn authenticate_workload(
    state: &AppState,
    headers: &HeaderMap,
    expected: Option<&str>,
) -> Result<(), ApiError> {
    let expected = expected
        .map(str::trim)
        .filter(|client| !client.is_empty())
        .ok_or(ApiError::Unauthorized)?;
    let verifier = state.bearer.as_ref().ok_or(ApiError::Unauthorized)?;
    let (_, azp) = verifier
        .verify_for_audience(presented(headers)?, INTERNAL_AUDIENCE)
        .await?;
    // The audience says the token is for this listener; `azp` says which client asked for it, and
    // one route belongs to one client. Another workload of the same realm is refused here, holding
    // a token that is otherwise perfectly valid.
    if azp.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

/// The Context Gateway, which reads the list of running workspace previews (PF-46).
pub async fn authenticate_gateway(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    authenticate_workload(state, headers, state.config.gateway_client_id.as_deref()).await
}

/// The credential proxy, which relays a run's own calls back to the Portal (AG-52). It held a
/// static bearer shared with the Portal until T-2271; a shared string is a key that never rotates
/// and that either side can leak, which CLAUDE.md rules out between cluster services.
pub async fn authenticate_agent_proxy(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    authenticate_workload(
        state,
        headers,
        state.config.agent_proxy_client_id.as_deref(),
    )
    .await
}

/// The client of a token for this listener, once it is verified as one: signed by the realm and
/// issued for [`INTERNAL_AUDIENCE`]. A Portal with no realm refuses every call.
async fn verified_client(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let verifier = state.bearer.as_ref().ok_or(ApiError::Unauthorized)?;
    let (_, azp) = verifier
        .verify_for_audience(presented(headers)?, INTERNAL_AUDIENCE)
        .await?;
    Ok(azp.unwrap_or_default())
}

/// The clients a stream of `project`'s pipeline `pipeline` presents on its sinks to this listener
/// (PL-19, PL-47): its project's `pipelines` account, which every stream of the project renders
/// with, and the pipeline's own federated client `{project}-pl-{pipeline}` once streams run as
/// their pipeline (T-1508).
pub fn pipeline_clients(project: &str, pipeline: &str) -> [String; 2] {
    use jc_core::kinds::{pipeline_identity, service_account::keycloak_client_id};
    [
        keycloak_client_id(project, crate::reconciler::streams::PIPELINE_ACCOUNT),
        keycloak_client_id(project, &pipeline_identity::account_name(pipeline)),
    ]
}

/// The clients a pipeline test or probe of `project` presents on its capture route (PL-43): the
/// project's `pipelines` account, and its harness principal `{project}-pl-pipeline-test` once the
/// harness runs through the token service (T-4050).
pub fn harness_clients(project: &str) -> [String; 2] {
    use jc_core::kinds::{pipeline_identity, service_account::keycloak_client_id};
    [
        keycloak_client_id(project, crate::reconciler::streams::PIPELINE_ACCOUNT),
        keycloak_client_id(
            project,
            &pipeline_identity::account_name(pipeline_identity::HARNESS),
        ),
    ]
}

/// The runner posting a record's outcome or rejection for `project`'s `pipeline` (T-3193): only a
/// client of that project's pipeline is admitted, so a project's runner client never posts for
/// another project. A refusal is logged with the client it saw, which is a name and not a secret.
pub async fn authenticate_pipeline_of(
    state: &AppState,
    headers: &HeaderMap,
    project: &str,
    pipeline: &str,
) -> Result<(), ApiError> {
    let client = verified_client(state, headers).await?;
    if pipeline_clients(project, pipeline).contains(&client) {
        return Ok(());
    }
    tracing::warn!(
        project = %project,
        pipeline = %pipeline,
        client = %client,
        "a pipeline sink was refused: the token is not of this project's pipeline"
    );
    Err(ApiError::Unauthorized)
}

/// The project's pipeline runner, which posts back what a pipeline test produced. The test's id is
/// 130 random bits and therefore a capability of its own, but a caller with no identity now gets
/// the 401 it deserves instead of the 404 that says only "no such test". `project` is the running
/// test's: its harness presents that project's `pipelines` client (T-3163) or its harness principal
/// (T-4050), and the client the configuration names for the runner is admitted as before.
pub async fn authenticate_pipeline_runner(
    state: &AppState,
    headers: &HeaderMap,
    project: Option<&str>,
) -> Result<(), ApiError> {
    let configured = state
        .config
        .pipeline_runner_client_id
        .as_deref()
        .map(str::trim)
        .filter(|client| !client.is_empty());
    let client = verified_client(state, headers).await?;
    let of_project = project.is_some_and(|project| harness_clients(project).contains(&client));
    if configured == Some(client.as_str()) || of_project {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-4050: a pipeline test's capture route admits its project's `pipelines` account and its
    /// harness principal, and no pipeline's own client and nothing of another project.
    #[test]
    fn a_test_is_known_by_its_projects_account_and_harness_principal() {
        let clients = harness_clients("praha");
        assert_eq!(
            clients,
            [
                "praha-pipelines".to_owned(),
                "praha-pl-pipeline-test".to_owned()
            ]
        );
        for other in [
            "helsinki-pl-pipeline-test",
            "praha-pl-odpad",
            "praha-pl-pipeline-tests",
            "",
        ] {
            assert!(!clients.contains(&other.to_owned()), "{other}");
        }
    }

    /// T-3193: a pipeline's sinks are admitted as its project's `pipelines` account or as the
    /// pipeline's own federated client, and as nothing of another project or pipeline.
    #[test]
    fn a_pipeline_is_known_by_its_projects_account_and_its_own_client() {
        assert_eq!(
            pipeline_clients("praha", "odpad"),
            ["praha-pipelines".to_owned(), "praha-pl-odpad".to_owned()]
        );
        let clients = pipeline_clients("praha", "odpad");
        for other in [
            "helsinki-pipelines",
            "praha-pl-other",
            "zilina-pl-odpad",
            "",
        ] {
            assert!(!clients.iter().any(|client| client == other), "{other:?}");
        }
    }
}
