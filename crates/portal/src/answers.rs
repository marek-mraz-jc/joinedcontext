//! The answers jc-assistant keeps for repeated questions are forgotten when a Policy or an
//! Endpoint changes, so nobody is served an answer read under rules that no longer hold (T-4731,
//! API/01 §34).

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::state::AppState;
use crate::store::Mirror;

/// The kinds whose change can make a kept answer wrong or no longer the asker's to read.
const RULING: [&str; 2] = ["Policy", "Endpoint"];

/// A digest of every mirrored Policy and Endpoint: it moves when any of them is added, removed
/// or edited, and only then.
pub fn fingerprint(mirror: &Mirror) -> u64 {
    let mut ruling: Vec<String> = mirror
        .matching(|env| RULING.contains(&env.kind.as_str()))
        .iter()
        .map(|env| {
            let key = env.key();
            format!("{}/{}/{}={}", key.namespace, key.kind, key.name, env.spec)
        })
        .collect();
    ruling.sort();
    let mut hash = DefaultHasher::new();
    ruling.hash(&mut hash);
    hash.finish()
}

/// Asks jc-assistant to forget the kept answers of every mirrored AssistantDeployment; how many
/// it forgot, `Some(0)` with no assistant, and `None` when one deployment could not be purged, so
/// the caller tries again rather than leave answers read under an old grant.
// ponytail: every deployment of every project, since a connector may read another project's
// Endpoint; per-deployment purges through resolve_connectors if Policy changes become frequent.
pub async fn forget_all(state: &AppState) -> Option<u64> {
    if state.config.knowledge_url.is_none() {
        return Some(0);
    }
    let (mut forgotten, mut all) = (0, true);
    for env in state
        .mirror
        .matching(|env| env.kind == "AssistantDeployment")
    {
        let key = env.key();
        let path = format!(
            "{}/knowledge/deployments/{}/answers",
            key.namespace, key.name
        );
        match crate::api::knowledge::ask(state, reqwest::Method::DELETE, &path, None).await {
            Ok((_, body)) => forgotten += body["forgotten"].as_u64().unwrap_or(0),
            // An assistant without the route keeps no answers to forget.
            Err(crate::error::ApiError::NotFound(_)) => {}
            Err(err) => {
                all = false;
                tracing::warn!(deployment = %path, error = %err, "the kept answers were not forgotten; again in 30 s")
            }
        }
    }
    all.then_some(forgotten)
}

/// How often [`spawn_periodic`] looks at the mirror in a running Portal.
pub(crate) const EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Looks at the mirror every `every` ([`EVERY`] in a running Portal) and forgets every kept
/// answer once a Policy or an Endpoint changed since the last look; a purge that failed is tried
/// again on the next look. Only the leader purges: the answers live in jc-assistant, and one
/// purge per change is enough.
pub(crate) fn spawn_periodic(state: AppState, every: std::time::Duration) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(every);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seen = fingerprint(&state.mirror);
        loop {
            interval.tick().await;
            if !state.leads() {
                continue;
            }
            let now = fingerprint(&state.mirror);
            if now != seen {
                if let Some(forgotten) = forget_all(&state).await {
                    tracing::info!(
                        forgotten,
                        "a Policy or an Endpoint changed: kept answers forgotten"
                    );
                    seen = now;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Portal with no assistant keeps no answers: nothing to forget, and nothing is asked.
    #[tokio::test]
    async fn a_portal_without_an_assistant_forgets_nothing() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        assert!(state.config.knowledge_url.is_none());
        assert_eq!(forget_all(&state).await, Some(0));
    }

    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ANSWERS: &str = "/internal/v1/projects/helsinki/knowledge/deployments/ask/answers";

    fn envelope(
        kind: &str,
        name: &str,
        spec: serde_json::Value,
    ) -> crate::resource::ResourceEnvelope {
        crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, "helsinki"),
            spec,
            status: None,
        }
    }

    /// A Portal whose assistant is `assistant`, with a realm that hands out the Portal's own
    /// token and one AssistantDeployment `helsinki/ask` in the mirror.
    async fn with_assistant(assistant: &MockServer) -> (AppState, MockServer) {
        let realm = crate::auth::oidc::tests::mock_realm().await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "portal-token", "token_type": "Bearer", "expires_in": 300
            })))
            .mount(&realm)
            .await;
        let mut config = crate::config::Config::for_tests();
        config.knowledge_url = Some(assistant.uri());
        let mut state = AppState::new(config, None);
        let client = crate::auth::oidc::OidcClient::discover(
            &crate::auth::oidc::tests::config_for(&realm),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery");
        state.oidc = Some(std::sync::Arc::new(client));
        state
            .mirror
            .upsert(envelope("AssistantDeployment", "ask", json!({})));
        (state, realm)
    }

    async fn deletes(assistant: &MockServer) -> usize {
        assistant
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "DELETE" && r.url.path() == ANSWERS)
            .count()
    }

    /// Waits up to 5 s for the assistant to have seen `want` purges.
    async fn until(assistant: &MockServer, want: usize) -> usize {
        let mut seen = deletes(assistant).await;
        for _ in 0..250 {
            if seen >= want {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            seen = deletes(assistant).await;
        }
        seen
    }

    /// The fingerprint moves with a Policy or an Endpoint added or edited, and never with any
    /// other kind.
    #[test]
    fn only_a_policy_or_an_endpoint_moves_the_fingerprint() {
        let mirror = Mirror::new();
        let empty = fingerprint(&mirror);
        mirror.upsert(envelope("App", "shop", json!({ "image": "a" })));
        assert_eq!(fingerprint(&mirror), empty, "an App rules no answer");
        mirror.upsert(envelope("Policy", "read", json!({ "rules": [] })));
        let with_policy = fingerprint(&mirror);
        assert_ne!(with_policy, empty);
        mirror.upsert(envelope("Policy", "read", json!({ "rules": ["x"] })));
        let edited = fingerprint(&mirror);
        assert_ne!(edited, with_policy, "an edited Policy moves it");
        mirror.upsert(envelope("Endpoint", "staff", json!({})));
        assert_ne!(fingerprint(&mirror), edited);
    }

    /// Each deployment's purge is counted; an assistant without the route forgets nothing and
    /// still counts as done; any other refusal leaves the purge undone (`None`).
    #[tokio::test]
    async fn forget_all_counts_every_purge_and_reports_a_refusal() {
        let assistant = MockServer::start().await;
        let (state, _realm) = with_assistant(&assistant).await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "forgotten": 4 })))
            .up_to_n_times(1)
            .mount(&assistant)
            .await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .up_to_n_times(1)
            .mount(&assistant)
            .await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "detail": "no route" })))
            .up_to_n_times(1)
            .mount(&assistant)
            .await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(500))
            .mount(&assistant)
            .await;
        assert_eq!(forget_all(&state).await, Some(4));
        assert_eq!(
            forget_all(&state).await,
            Some(0),
            "no count reads as none forgotten"
        );
        assert_eq!(forget_all(&state).await, Some(0), "a 404 keeps no answers");
        assert_eq!(forget_all(&state).await, None, "a 500 is tried again");
    }

    /// The loop purges once per change of a Policy, and a purge the assistant refused is tried
    /// again on the next look until it lands; with nothing changed it asks nothing.
    #[tokio::test]
    async fn the_loop_purges_once_per_change_and_retries_a_refusal() {
        let assistant = MockServer::start().await;
        let (state, _realm) = with_assistant(&assistant).await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&assistant)
            .await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "forgotten": 2 })))
            .mount(&assistant)
            .await;
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = Log(log.clone());
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .with_writer(move || writer.clone())
                .finish(),
        );
        spawn_periodic(state.clone(), std::time::Duration::from_millis(10));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            deletes(&assistant).await,
            0,
            "nothing changed, nothing purged"
        );
        state
            .mirror
            .upsert(envelope("Policy", "read", json!({ "rules": [] })));
        assert_eq!(
            until(&assistant, 2).await,
            2,
            "the refused purge is tried again"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            deletes(&assistant).await,
            2,
            "a landed purge is not repeated"
        );
        std::io::Write::flush(&mut Log(log.clone())).expect("an in-memory log flushes");
        let text = String::from_utf8(log.lock().expect("log").clone()).expect("utf-8");
        assert!(text.contains("again in 30 s"), "{text}");
        assert!(
            text.contains("forgotten=2") && text.contains("kept answers forgotten"),
            "{text}"
        );
    }

    /// The log the purge loop writes, kept for the test to read.
    #[derive(Clone)]
    struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Log {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A follower replica never purges: the leader does it once for every replica.
    #[tokio::test]
    async fn a_follower_never_purges() {
        let assistant = MockServer::start().await;
        let (state, _realm) = with_assistant(&assistant).await;
        Mock::given(method("DELETE"))
            .and(path(ANSWERS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "forgotten": 1 })))
            .mount(&assistant)
            .await;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://follower@127.0.0.1:1/none")
            .expect("a lazy pool");
        let shako = std::sync::Arc::new(
            crate::forge::ShakoClient::new(
                "http://localhost:3000".parse().expect("a url"),
                "test-owner",
                "test-repo",
                "token",
            )
            .expect("a client"),
        );
        let follower = crate::reconciler::Syncer::new(shako, std::sync::Arc::new(Mirror::new()))
            .with_leadership(std::sync::Arc::new(
                crate::reconciler::Leadership::reconciler(pool),
            ));
        let state = state.with_syncer(std::sync::Arc::new(follower));
        assert!(!state.leads());
        spawn_periodic(state.clone(), std::time::Duration::from_millis(10));
        state
            .mirror
            .upsert(envelope("Endpoint", "staff", json!({})));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(deletes(&assistant).await, 0);
    }
}
