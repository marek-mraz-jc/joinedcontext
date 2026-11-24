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
    use crate::config::Config;
    use axum::http::HeaderValue;
    use serde_json::json;
    use std::sync::Arc;

    const ISSUER: &str = "https://idm.example.test/realms/dev";

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    /// A Portal with a realm whose one key signs `sign`'s tokens, and the clients of the gateway,
    /// the credential proxy and the runner named as in a deployment.
    fn world() -> (AppState, impl Fn(&str, &str) -> HeaderMap) {
        let mut config = Config::for_tests();
        config.gateway_client_id = Some("context-gateway".into());
        config.agent_proxy_client_id = Some(" credential-proxy ".into());
        config.pipeline_runner_client_id = Some("pipeline-runner".into());
        let mut state = AppState::new(config, None);
        let (verifier, sign) = crate::auth::bearer::test_verifier(ISSUER, "portal-api", "k1");
        state.bearer = Some(Arc::new(verifier));
        let headers = move |audience: &str, azp: &str| {
            let token = sign(&json!({
                "iss": ISSUER, "typ": "Bearer", "aud": audience, "sub": "svc", "azp": azp,
                "exp": now() + 60, "iat": now(),
            }));
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
            );
            headers
        };
        (state, headers)
    }

    fn bearer(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(value).expect("header"),
        );
        headers
    }

    /// No header, another scheme, an empty or blank token and a header that is not text are each
    /// a 401 before any key is looked at.
    #[test]
    fn only_a_non_blank_bearer_is_presented() {
        assert!(matches!(
            presented(&HeaderMap::new()),
            Err(ApiError::Unauthorized)
        ));
        for refused in ["Basic eDp5", "Bearer ", "Bearer    ", "bearer abc", "abc"] {
            assert!(
                matches!(presented(&bearer(refused)), Err(ApiError::Unauthorized)),
                "{refused:?}"
            );
        }
        let mut opaque = HeaderMap::new();
        opaque.insert(
            header::AUTHORIZATION,
            HeaderValue::from_bytes(b"Bearer \xff\xfe").expect("bytes"),
        );
        assert!(matches!(presented(&opaque), Err(ApiError::Unauthorized)));
        assert_eq!(presented(&bearer("Bearer abc.def")).ok(), Some("abc.def"));
    }

    /// PF-46, AG-52: each door opens to its own client's internal token and to nothing else: not
    /// another workload of the realm, not a token for the Portal's API, not a forged one.
    #[tokio::test]
    async fn each_internal_door_admits_its_own_client_alone() {
        let (state, token) = world();
        let internal = INTERNAL_AUDIENCE;
        assert!(
            authenticate_gateway(&state, &token(internal, "context-gateway"))
                .await
                .is_ok()
        );
        // The configured name is trimmed: " credential-proxy " names the client `credential-proxy`.
        assert!(
            authenticate_agent_proxy(&state, &token(internal, "credential-proxy"))
                .await
                .is_ok()
        );
        for (audience, azp) in [
            (internal, "credential-proxy"),
            (internal, "pipeline-runner"),
            (internal, ""),
            ("portal-api", "context-gateway"),
        ] {
            assert!(
                matches!(
                    authenticate_gateway(&state, &token(audience, azp)).await,
                    Err(ApiError::Unauthorized)
                ),
                "{audience} {azp}"
            );
        }
        assert!(matches!(
            authenticate_agent_proxy(&state, &token(internal, "context-gateway")).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            authenticate_gateway(&state, &bearer("Bearer not-a-jwt")).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            authenticate_gateway(&state, &HeaderMap::new()).await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// A token without `azp` names no client and opens no door.
    #[tokio::test]
    async fn a_token_naming_no_client_is_refused() {
        let mut config = Config::for_tests();
        config.gateway_client_id = Some("context-gateway".into());
        let mut state = AppState::new(config, None);
        let (verifier, sign) = crate::auth::bearer::test_verifier(ISSUER, "portal-api", "k1");
        state.bearer = Some(Arc::new(verifier));
        let token = sign(&json!({
            "iss": ISSUER, "typ": "Bearer", "aud": INTERNAL_AUDIENCE, "sub": "svc", "exp": now() + 60, "iat": now(),
        }));
        let headers = bearer(&format!("Bearer {token}"));
        assert!(matches!(
            authenticate_gateway(&state, &headers).await,
            Err(ApiError::Unauthorized)
        ));
        // The runner's door reads the missing client as "", which no configuration names.
        assert!(matches!(
            authenticate_pipeline_runner(&state, &headers, Some("praha")).await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// Fail closed: a route whose client is not configured (absent or blank) and a Portal with no
    /// realm refuse a token that would otherwise be valid.
    #[tokio::test]
    async fn an_unconfigured_door_or_a_portal_without_realm_refuses_everyone() {
        let (state, token) = world();
        let headers = token(INTERNAL_AUDIENCE, "context-gateway");
        for expected in [None, Some(""), Some("   ")] {
            assert!(
                matches!(
                    authenticate_workload(&state, &headers, expected).await,
                    Err(ApiError::Unauthorized)
                ),
                "{expected:?}"
            );
        }
        let mut no_realm = state.clone();
        no_realm.bearer = None;
        assert!(matches!(
            authenticate_gateway(&no_realm, &headers).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            authenticate_pipeline_of(&no_realm, &headers, "praha", "odpad").await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            authenticate_pipeline_runner(&no_realm, &headers, None).await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// T-3193: a pipeline's sink admits its project's `pipelines` account and the pipeline's own
    /// client, and refuses another project's or another pipeline's client and a non-internal token.
    #[tokio::test]
    async fn a_pipeline_sink_admits_its_projects_clients_alone() {
        let (state, token) = world();
        for client in ["praha-pipelines", "praha-pl-odpad"] {
            assert!(
                authenticate_pipeline_of(
                    &state,
                    &token(INTERNAL_AUDIENCE, client),
                    "praha",
                    "odpad"
                )
                .await
                .is_ok(),
                "{client}"
            );
        }
        for (audience, client) in [
            (INTERNAL_AUDIENCE, "helsinki-pipelines"),
            (INTERNAL_AUDIENCE, "praha-pl-other"),
            (INTERNAL_AUDIENCE, "pipeline-runner"),
            ("portal-api", "praha-pipelines"),
        ] {
            assert!(
                matches!(
                    authenticate_pipeline_of(&state, &token(audience, client), "praha", "odpad")
                        .await,
                    Err(ApiError::Unauthorized)
                ),
                "{audience} {client}"
            );
        }
    }

    /// T-3193: a refused sink is logged with the client it saw and never with the token.
    #[tokio::test]
    async fn a_refused_sink_is_logged_with_its_client_and_never_its_token() {
        let log = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = {
            let log = Arc::clone(&log);
            move || Captured(Arc::clone(&log))
        };
        let _logging = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(writer)
                .with_ansi(false)
                .finish(),
        );
        let (state, token) = world();
        let headers = token(INTERNAL_AUDIENCE, "helsinki-pipelines");
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim_start_matches("Bearer ").to_owned())
            .expect("a token");
        assert!(authenticate_pipeline_of(&state, &headers, "praha", "odpad")
            .await
            .is_err());
        std::io::Write::flush(&mut Captured(Arc::clone(&log))).expect("flush");
        let logged = String::from_utf8(log.lock().expect("log").clone()).expect("utf-8");
        assert!(logged.contains("a pipeline sink was refused"), "{logged}");
        assert!(logged.contains("client=helsinki-pipelines"), "{logged}");
        assert!(
            logged.contains("project=praha") && logged.contains("pipeline=odpad"),
            "{logged}"
        );
        assert!(
            !logged.contains(&presented),
            "the token is logged: {logged}"
        );
    }

    /// A log sink the test reads back.
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log").extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// T-3163, T-4050: the runner's capture route admits the configured runner for any test, and a
    /// project's harness clients only for that project's test.
    #[tokio::test]
    async fn the_capture_route_admits_the_runner_and_the_tests_own_harness() {
        let (state, token) = world();
        let runner = token(INTERNAL_AUDIENCE, "pipeline-runner");
        assert!(authenticate_pipeline_runner(&state, &runner, None)
            .await
            .is_ok());
        assert!(authenticate_pipeline_runner(&state, &runner, Some("praha"))
            .await
            .is_ok());
        for client in ["praha-pipelines", "praha-pl-pipeline-test"] {
            let headers = token(INTERNAL_AUDIENCE, client);
            assert!(
                authenticate_pipeline_runner(&state, &headers, Some("praha"))
                    .await
                    .is_ok(),
                "{client}"
            );
            assert!(matches!(
                authenticate_pipeline_runner(&state, &headers, Some("helsinki")).await,
                Err(ApiError::Unauthorized)
            ));
            assert!(matches!(
                authenticate_pipeline_runner(&state, &headers, None).await,
                Err(ApiError::Unauthorized)
            ));
        }
        // A blank runner configuration names no client, so a token with the blank name is no runner.
        let mut blank = state.clone();
        let mut config = (*blank.config).clone();
        config.pipeline_runner_client_id = Some("  ".into());
        blank.config = Arc::new(config);
        assert!(matches!(
            authenticate_pipeline_runner(&blank, &runner, None).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            authenticate_pipeline_runner(&state, &token("portal-api", "pipeline-runner"), None)
                .await,
            Err(ApiError::Unauthorized)
        ));
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
