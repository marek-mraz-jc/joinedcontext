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
use crate::store::Mirror;

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
pub fn pipeline_clients(mirror: &Mirror, project: &str, pipeline: &str) -> [String; 2] {
    use jc_core::kinds::pipeline_identity;
    [
        mirror.client_id(project, crate::reconciler::streams::PIPELINE_ACCOUNT),
        mirror.client_id(project, &pipeline_identity::account_name(pipeline)),
    ]
}

/// The clients a pipeline test or probe of `project` presents on its capture route (PL-43): the
/// project's `pipelines` account, and its harness principal `{project}-pl-pipeline-test` once the
/// harness runs through the token service (T-4050).
pub fn harness_clients(mirror: &Mirror, project: &str) -> [String; 2] {
    use jc_core::kinds::pipeline_identity;
    [
        mirror.client_id(project, crate::reconciler::streams::PIPELINE_ACCOUNT),
        mirror.client_id(
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
    if pipeline_clients(&state.mirror, project, pipeline).contains(&client) {
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
    let of_project =
        project.is_some_and(|project| harness_clients(&state.mirror, project).contains(&client));
    if configured == Some(client.as_str()) || of_project {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::bearer::tests::{keypair, now, sign, ISSUER};
    use crate::auth::bearer::BearerVerifier;
    use crate::config::Config;
    use std::sync::Arc;

    /// A Portal whose realm signs with one key, the internal clients named as a deployment
    /// names them, and the signer for that realm's tokens.
    fn portal() -> (AppState, jsonwebtoken::EncodingKey) {
        let mut config = Config::for_tests();
        config.gateway_client_id = Some("context-gateway".into());
        config.agent_proxy_client_id = Some("agent-proxy".into());
        config.pipeline_runner_client_id = Some("pipeline-runner".into());
        let mut state = AppState::new(config, None);
        let (signer, set) = keypair("k1");
        // Port 9 refuses at once: an unknown kid fails fast instead of reaching a realm.
        let verifier = BearerVerifier::with_jwks_url(
            &url::Url::parse(ISSUER).expect("issuer"),
            "portal-api",
            url::Url::parse("http://127.0.0.1:9/certs").expect("jwks"),
        );
        assert_eq!(verifier.install(&set), 1);
        state.bearer = Some(Arc::new(verifier));
        (state, signer)
    }

    /// `Authorization: Bearer` with a token for `audience` obtained by `azp` (none when `None`).
    fn bearer(signer: &jsonwebtoken::EncodingKey, audience: &str, azp: Option<&str>) -> HeaderMap {
        let mut claims = serde_json::json!({
            "iss": ISSUER, "typ": "Bearer", "aud": audience, "sub": "service-account",
            "exp": now() + 60, "iat": now(),
        });
        if let Some(azp) = azp {
            claims["azp"] = serde_json::json!(azp);
        }
        authorization(&format!("Bearer {}", sign(signer, "k1", &claims)))
    }

    /// `Err(UNAUTHORIZED)` is a 401 refusal; any other error reads `Err(false)`.
    const UNAUTHORIZED: bool = true;

    /// The refusal as whether it is the 401, so verdicts compare (`ApiError` is not `PartialEq`).
    fn code(error: ApiError) -> bool {
        matches!(error, ApiError::Unauthorized)
    }

    fn authorization(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value.parse().expect("header"));
        headers
    }

    /// Only `Bearer <token>` with a non-blank token is a presented token; any other shape is 401.
    #[test]
    fn only_a_non_blank_bearer_is_presented() {
        assert_eq!(
            presented(&authorization("Bearer abc")).map_err(code),
            Ok("abc")
        );
        for refused in [
            "Bearer ",
            "Bearer    ",
            "Basic abc",
            "bearer abc",
            "abc",
            "Bearer",
        ] {
            assert_eq!(
                presented(&authorization(refused)).map_err(code),
                Err(UNAUTHORIZED),
                "{refused:?}"
            );
        }
        assert_eq!(
            presented(&HeaderMap::new()).map_err(code),
            Err(UNAUTHORIZED)
        );
        let mut opaque = HeaderMap::new();
        opaque.insert(
            header::AUTHORIZATION,
            axum::http::HeaderValue::from_bytes(b"Bearer \xff").expect("bytes"),
        );
        assert_eq!(presented(&opaque).map_err(code), Err(UNAUTHORIZED));
    }

    /// PF-46: a route's own client with a token for this listener passes; the same realm's
    /// other workload, a token for the API audience, one with no client and none at all do not.
    #[tokio::test]
    async fn a_route_opens_for_its_own_client_and_this_listener_alone() {
        let (state, signer) = portal();
        let own = bearer(&signer, INTERNAL_AUDIENCE, Some("context-gateway"));
        assert_eq!(
            authenticate_workload(&state, &own, Some("context-gateway"))
                .await
                .map_err(code),
            Ok(())
        );
        assert_eq!(
            authenticate_workload(&state, &own, Some(" context-gateway "))
                .await
                .map_err(code),
            Ok(())
        );
        for (headers, why) in [
            (
                bearer(&signer, INTERNAL_AUDIENCE, Some("agent-proxy")),
                "another client",
            ),
            (
                bearer(&signer, "portal-api", Some("context-gateway")),
                "the API audience",
            ),
            (bearer(&signer, INTERNAL_AUDIENCE, None), "no client"),
            (HeaderMap::new(), "no token"),
            (authorization("Bearer not-a-jwt"), "garbled token"),
        ] {
            assert_eq!(
                authenticate_workload(&state, &headers, Some("context-gateway"))
                    .await
                    .map_err(code),
                Err(UNAUTHORIZED),
                "{why}"
            );
        }
    }

    /// Fail closed: a route nobody configured, a blank client and a Portal with no realm refuse
    /// even a valid token.
    #[tokio::test]
    async fn an_unconfigured_route_or_realm_refuses_a_valid_token() {
        let (mut state, signer) = portal();
        let token = bearer(&signer, INTERNAL_AUDIENCE, Some("context-gateway"));
        for expected in [None, Some(""), Some("   ")] {
            assert_eq!(
                authenticate_workload(&state, &token, expected)
                    .await
                    .map_err(code),
                Err(UNAUTHORIZED),
                "{expected:?}"
            );
        }
        state.bearer = None;
        assert_eq!(
            authenticate_workload(&state, &token, Some("context-gateway"))
                .await
                .map_err(code),
            Err(UNAUTHORIZED)
        );
        assert_eq!(
            authenticate_pipeline_runner(
                &state,
                &bearer(&signer, INTERNAL_AUDIENCE, Some("pipeline-runner")),
                None
            )
            .await
            .map_err(code),
            Err(UNAUTHORIZED)
        );
    }

    /// T-2271: the gateway's door and the credential proxy's door each take their own client
    /// and refuse the other's token.
    #[tokio::test]
    async fn the_gateway_and_the_agent_proxy_do_not_open_each_others_door() {
        let (mut state, signer) = portal();
        let gateway = bearer(&signer, INTERNAL_AUDIENCE, Some("context-gateway"));
        let proxy = bearer(&signer, INTERNAL_AUDIENCE, Some("agent-proxy"));
        assert_eq!(
            authenticate_gateway(&state, &gateway).await.map_err(code),
            Ok(())
        );
        assert_eq!(
            authenticate_agent_proxy(&state, &proxy).await.map_err(code),
            Ok(())
        );
        assert_eq!(
            authenticate_gateway(&state, &proxy).await.map_err(code),
            Err(UNAUTHORIZED)
        );
        assert_eq!(
            authenticate_agent_proxy(&state, &gateway)
                .await
                .map_err(code),
            Err(UNAUTHORIZED)
        );
        Arc::make_mut(&mut state.config).gateway_client_id = None;
        assert_eq!(
            authenticate_gateway(&state, &gateway).await.map_err(code),
            Err(UNAUTHORIZED)
        );
    }

    /// T-3193: a pipeline's sink takes its project's `pipelines` account and the pipeline's own
    /// client; another project's or pipeline's client, a token with no client and the API
    /// audience are refused.
    #[tokio::test]
    async fn a_pipeline_sink_takes_only_its_own_projects_pipeline_clients() {
        // The refusal's log line names the client it saw, never the token.
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = {
            let log = std::sync::Arc::clone(&log);
            move || Captured(std::sync::Arc::clone(&log))
        };
        let _logging = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(writer)
                .with_ansi(false)
                .finish(),
        );
        let (state, signer) = portal();
        for client in ["praha-pipelines", "praha-pl-odpad"] {
            let headers = bearer(&signer, INTERNAL_AUDIENCE, Some(client));
            assert_eq!(
                authenticate_pipeline_of(&state, &headers, "praha", "odpad")
                    .await
                    .map_err(code),
                Ok(()),
                "{client}"
            );
        }
        for (headers, why) in [
            (
                bearer(&signer, INTERNAL_AUDIENCE, Some("zilina-pipelines")),
                "another project",
            ),
            (
                bearer(&signer, INTERNAL_AUDIENCE, Some("praha-pl-other")),
                "another pipeline",
            ),
            (
                bearer(&signer, INTERNAL_AUDIENCE, Some("pipeline-runner")),
                "the runner",
            ),
            (bearer(&signer, INTERNAL_AUDIENCE, None), "no client"),
            (
                bearer(&signer, "portal-api", Some("praha-pipelines")),
                "the API audience",
            ),
            (HeaderMap::new(), "no token"),
        ] {
            let token = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim_start_matches("Bearer ").to_owned());
            assert_eq!(
                authenticate_pipeline_of(&state, &headers, "praha", "odpad")
                    .await
                    .map_err(code),
                Err(UNAUTHORIZED),
                "{why}"
            );
            let logged = String::from_utf8(log.lock().expect("log").clone()).expect("utf-8");
            assert!(
                token.is_none_or(|token| !logged.contains(&token)),
                "{why}: the token is logged"
            );
        }
        std::io::Write::flush(&mut Captured(std::sync::Arc::clone(&log))).expect("flush");
        let logged = String::from_utf8(log.lock().expect("log").clone()).expect("utf-8");
        assert!(logged.contains("a pipeline sink was refused"), "{logged}");
        for client in ["zilina-pipelines", "praha-pl-other", "pipeline-runner"] {
            assert!(
                logged.contains(&format!("client={client}")),
                "{client}: {logged}"
            );
        }
    }

    /// A log sink the test reads back.
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log").extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// T-3163, T-4050: the runner's capture route takes the configured runner client and the
    /// running test's project harness clients; another project's, a blank configuration and a
    /// test of no project do not widen it.
    #[tokio::test]
    async fn the_runner_route_takes_the_configured_client_and_its_projects_harness() {
        let (mut state, signer) = portal();
        let runner = bearer(&signer, INTERNAL_AUDIENCE, Some("pipeline-runner"));
        let harness = bearer(&signer, INTERNAL_AUDIENCE, Some("praha-pl-pipeline-test"));
        let account = bearer(&signer, INTERNAL_AUDIENCE, Some("praha-pipelines"));
        assert_eq!(
            authenticate_pipeline_runner(&state, &runner, None)
                .await
                .map_err(code),
            Ok(())
        );
        assert_eq!(
            authenticate_pipeline_runner(&state, &runner, Some("praha"))
                .await
                .map_err(code),
            Ok(())
        );
        assert_eq!(
            authenticate_pipeline_runner(&state, &harness, Some("praha"))
                .await
                .map_err(code),
            Ok(())
        );
        assert_eq!(
            authenticate_pipeline_runner(&state, &account, Some("praha"))
                .await
                .map_err(code),
            Ok(())
        );
        for (headers, project, why) in [
            (&harness, None, "a harness with no running test's project"),
            (&harness, Some("zilina"), "another project's test"),
            (&account, Some("zilina"), "another project's account"),
        ] {
            assert_eq!(
                authenticate_pipeline_runner(&state, headers, project)
                    .await
                    .map_err(code),
                Err(UNAUTHORIZED),
                "{why}"
            );
        }
        for blank in [None, Some("  ".to_owned())] {
            Arc::make_mut(&mut state.config).pipeline_runner_client_id = blank;
            assert_eq!(
                authenticate_pipeline_runner(&state, &runner, Some("praha"))
                    .await
                    .map_err(code),
                Err(UNAUTHORIZED)
            );
        }
        // A token with no client never equals a blank configuration.
        Arc::make_mut(&mut state.config).pipeline_runner_client_id = Some(String::new());
        assert_eq!(
            authenticate_pipeline_runner(&state, &bearer(&signer, INTERNAL_AUDIENCE, None), None)
                .await
                .map_err(code),
            Err(UNAUTHORIZED)
        );
    }

    /// T-4050: a pipeline test's capture route admits its project's `pipelines` account and its
    /// harness principal, and no pipeline's own client and nothing of another project.
    #[test]
    fn a_test_is_known_by_its_projects_account_and_harness_principal() {
        let clients = harness_clients(&Mirror::new(), "praha");
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
            pipeline_clients(&Mirror::new(), "praha", "odpad"),
            ["praha-pipelines".to_owned(), "praha-pl-odpad".to_owned()]
        );
        let clients = pipeline_clients(&Mirror::new(), "praha", "odpad");
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
