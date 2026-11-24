//! Resolves run parameters and permissions from Portal backend.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use url::Url;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunContext {
    pub id: String,
    pub project: String,
    pub app_name: String,
    pub endpoint_slug: String,
    /// Every endpoint slug of the run, the primary first (AP-44): what
    /// `/v1/data/endpoints/{slug}/…` may address. A Portal that sends none means the primary alone.
    #[serde(default)]
    pub endpoint_slugs: Vec<String>,
    pub allows_write: bool,
    pub branch: String,
    /// The folder the run writes, as its repository spells it.
    pub path_prefix: String,
    /// The project's own repository in layout 2, in the configuration repository's owner: the
    /// one repository the forge route reaches for this run (AG-86, CC-87). Absent, the
    /// configuration repository.
    #[serde(default)]
    pub repository: Option<String>,
    pub status: String,
    /// The run's kind (`conversation`, `application`, …): whose daily budget its calls count
    /// against (AG-97). A Portal that does not send it counts as `other`.
    #[serde(default)]
    pub kind: String,
    pub ticket_hash: String,
    pub max_tokens: u64,
    pub allowed_hosts: Vec<String>,
    pub requests_per_minute: u32,
    /// Model calls this run may make, from the profile's `limits.stepsPerRun` (AG-25, AG-51);
    /// `0` when the Portal does not send it.
    #[serde(default)]
    pub steps_per_run: u32,
    pub max_response_bytes: u64,
    /// Bytes this run may read from the allow-listed hosts in total, from the profile's
    /// `egress.maxBytesPerRun` (AG-65). `0` — a Portal that does not send it, or a profile that
    /// names no host — is a run that reaches nothing.
    #[serde(default)]
    pub max_egress_bytes_per_run: u64,
    pub created_by: String,
    /// The Organization of the run's project, when it has one (T-7144): the one Organization
    /// whose own realm may issue the token of the person who started it.
    #[serde(default)]
    pub organization: Option<String>,
    pub model_name: String,
    /// The profile's `model.reasoningEffort`, absent when it names none (AG-72).
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

impl RunContext {
    /// The endpoints this run may read through the proxy, the primary first.
    pub fn slugs(&self) -> Vec<&str> {
        if self.endpoint_slugs.is_empty() {
            return vec![self.endpoint_slug.as_str()];
        }
        self.endpoint_slugs.iter().map(String::as_str).collect()
    }
}

/// Run records the proxy has fetched, with the moment each was fetched.
type RunCache = Arc<RwLock<HashMap<String, (Instant, Arc<RunContext>)>>>;

#[derive(Clone)]
pub struct RunResolver {
    http: reqwest::Client,
    portal_base: Url,
    /// How the resolver names itself to the Portal (AG-52, T-2271): its own client's token, minted
    /// and held by the credential manager. `None` is a resolver in a test, which asks nobody.
    credentials: Option<crate::inject::CredentialManager>,
    cache: RunCache,
    /// The tickets that verified, per run, so a run's later calls skip Argon2 (F-PLT1-007).
    pub tickets: crate::tickets::TicketCache,
}

impl std::fmt::Debug for RunResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunResolver")
            .field("portal_base", &self.portal_base.as_str())
            .field("credentials", &self.credentials.is_some())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("run not found: {0}")]
    NotFound(String),
    #[error("run is in terminal state '{0}' and cannot execute")]
    NotActive(String),
    #[error("portal communication failure: {0}")]
    Transport(String),
}

impl RunResolver {
    pub fn new(portal_base: Url, credentials: crate::inject::CredentialManager) -> Self {
        Self {
            // The lookup presents this proxy's own Portal token; a redirect would carry it to
            // whatever the Portal named (T-1695).
            http: crate::no_redirect_client(Some(Duration::from_secs(5))),
            portal_base,
            credentials: Some(credentials),
            cache: Arc::new(RwLock::new(HashMap::new())),
            tickets: crate::tickets::TicketCache::default(),
        }
    }

    pub fn with_cached(run: RunContext) -> Self {
        let mut map = HashMap::new();
        map.insert(
            run.id.clone(),
            (Instant::now() + Duration::from_secs(3600), Arc::new(run)),
        );
        Self {
            http: crate::no_redirect_client(Some(Duration::from_secs(5))),
            portal_base: Url::parse("http://portal").unwrap(),
            credentials: None,
            cache: Arc::new(RwLock::new(map)),
            tickets: crate::tickets::TicketCache::default(),
        }
    }

    /// A resolver that holds `run` for an hour and asks `portal_base` when it must look past it.
    pub fn with_cached_at(portal_base: Url, run: RunContext) -> Self {
        let mut resolver = Self::with_cached(run);
        resolver.portal_base = portal_base;
        resolver
    }

    pub async fn resolve(&self, run_id: &str) -> Result<Arc<RunContext>, RunError> {
        let now = Instant::now();
        {
            let cache = self.cache.read().await;
            if let Some((exp, run)) = cache.get(run_id) {
                if *exp > now {
                    Self::check_status(run)?;
                    return Ok(Arc::clone(run));
                }
            }
        }

        self.fetch(run_id).await
    }

    /// The run as the Portal holds it now, past the cache: a conversation gains an endpoint
    /// between two calls, and its first call through that endpoint must not wait out the cache
    /// (AG-75). A failure leaves the cached record as it was.
    pub async fn resolve_fresh(&self, run_id: &str) -> Result<Arc<RunContext>, RunError> {
        self.fetch(run_id).await
    }

    async fn fetch(&self, run_id: &str) -> Result<Arc<RunContext>, RunError> {
        let now = Instant::now();
        let url = self.lookup_url(run_id)?;

        let mut req = self.http.get(url);
        if let Some(credentials) = &self.credentials {
            let bearer = credentials
                .get_portal_token()
                .await
                .map_err(RunError::Transport)?;
            req = req.bearer_auth(bearer);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| RunError::Transport(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RunError::NotFound(run_id.to_string()));
        }
        if !resp.status().is_success() {
            return Err(RunError::Transport(format!("status {}", resp.status())));
        }

        let run: RunContext = resp
            .json()
            .await
            .map_err(|e| RunError::Transport(e.to_string()))?;
        Self::check_status(&run)?;

        let arc = Arc::new(run);
        let mut cache = self.cache.write().await;
        cache.insert(
            run_id.to_string(),
            (now + Duration::from_secs(5), Arc::clone(&arc)),
        );

        Ok(arc)
    }

    /// Where the Portal holds one run, with the id as one path segment and nothing else.
    ///
    /// The id arrives in a header a workspace writes. `Url::set_path` takes what it is given as a
    /// path, so an id shaped like one (`../../…`, `%2e%2e/…`) used to steer this lookup — which
    /// carries this proxy's own Portal token — at another Portal route entirely (T-1695). The id
    /// is checked at the door by `auth::run_id_is_well_formed`; here it is appended as a single
    /// percent-encoded segment, so no shape of it can be read as a path however this resolver is
    /// called.
    fn lookup_url(&self, run_id: &str) -> Result<Url, RunError> {
        let mut url = self.portal_base.clone();
        url.set_path("");
        url.path_segments_mut()
            .map_err(|_| RunError::Transport("the Portal base cannot hold a path".to_owned()))?
            .clear()
            .push("internal")
            .push("agent-runs")
            .push(run_id);
        Ok(url)
    }

    fn check_status(run: &RunContext) -> Result<(), RunError> {
        match run.status.as_str() {
            "failed" | "cancelled" | "expired" | "published" => {
                Err(RunError::NotActive(run.status.clone()))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: &str) -> RunContext {
        serde_json::from_value(serde_json::json!({
            "id": "run-1", "project": "p", "appName": "a", "endpointSlug": "e",
            "allowsWrite": false, "branch": "b", "pathPrefix": "", "status": status,
            "ticketHash": "$argon2id$secret-hash", "maxTokens": 1, "allowedHosts": [],
            "requestsPerMinute": 1, "maxResponseBytes": 1, "createdBy": "u", "modelName": "m"
        }))
        .expect("a run")
    }

    /// A resolver's debug form names the Portal and whether it holds credentials, and nothing
    /// of any run it caches (T-7795).
    #[test]
    fn the_debug_form_names_the_portal_and_no_run() {
        let shown = format!("{:?}", RunResolver::with_cached(run("building")));
        assert!(shown.contains("http://portal/"), "{shown}");
        assert!(shown.contains("credentials: false"), "{shown}");
        assert!(!shown.contains("argon2"), "{shown}");
    }

    /// A cached record past its time is asked for again: a Portal that cannot be reached is
    /// then the platform's failure, not the stale record (T-7795).
    #[tokio::test]
    async fn an_expired_record_is_asked_for_again() {
        let resolver = RunResolver::with_cached_at(
            Url::parse("http://portal.invalid:8080").expect("a URL"),
            run("building"),
        );
        resolver.cache.write().await.insert(
            "run-1".to_owned(),
            (
                Instant::now() - Duration::from_secs(1),
                Arc::new(run("building")),
            ),
        );
        assert!(matches!(
            resolver.resolve("run-1").await,
            Err(RunError::Transport(_))
        ));
    }

    /// A record the Portal answered is held for five seconds: the next resolve inside them asks
    /// the Portal nothing (T-7795).
    #[tokio::test]
    async fn an_answered_record_is_held_for_its_five_seconds() {
        let portal = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "run-1", "project": "p", "appName": "a", "endpointSlug": "e",
                    "allowsWrite": false, "branch": "b", "pathPrefix": "", "status": "building",
                    "ticketHash": "", "maxTokens": 1, "allowedHosts": [], "requestsPerMinute": 1,
                    "maxResponseBytes": 1, "createdBy": "u", "modelName": "m"
                })),
            )
            .expect(1)
            .mount(&portal)
            .await;
        let resolver =
            RunResolver::with_cached_at(Url::parse(&portal.uri()).expect("a URL"), run("building"));
        resolver.cache.write().await.clear();
        for _ in 0..2 {
            assert_eq!(
                resolver.resolve("run-1").await.expect("the run").id,
                "run-1"
            );
        }
    }

    /// A Portal base that cannot hold a path asks nobody and says so (T-7795).
    #[tokio::test]
    async fn a_portal_base_that_cannot_hold_a_path_asks_nobody() {
        let resolver = RunResolver::with_cached_at(
            Url::parse("mailto:portal@example.invalid").expect("a URL"),
            run("building"),
        );
        match resolver.resolve_fresh("run-1").await {
            Err(RunError::Transport(why)) => assert!(why.contains("cannot hold a path"), "{why}"),
            other => panic!("{other:?}"),
        }
    }
}
