//! Shared application state: configuration, the discovered OIDC client and the
//! back-channel logout marks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;

use crate::agents::events::AgentEventHub;
use crate::agents::store::AgentStore;
use crate::auth::bearer::BearerVerifier;
use crate::auth::oidc::OidcClient;
use crate::auth::session::Session;
use crate::branding::Branding;
use crate::config::Config;
use crate::git::GiteaClient;
use crate::ops::drafts::{DraftHub, DraftStore};
use crate::reconciler::{Leadership, StreamDeployer, Syncer};
use crate::store::Mirror;

#[derive(Clone)]
pub struct AppState {
    /// The apps database and bucket of the `wasm` Apps; `None` where none is configured, and a
    /// `wasm` App's publish then says so (AP-149).
    pub wasm_apps: Option<Arc<crate::apps::wasm_apps::WasmApps>>,
    /// The Smart Data Models mirror in the image (T-6353), `None` where imports go to Model Tools.
    pub sdm_mirror: Option<Arc<crate::tools::sdm_mirror::SdmMirror>>,
    pub config: Arc<Config>,
    /// The client every open-data catalogue search goes out through: the platform's outbound
    /// guard ([`crate::api::catalogue::ckan_client`]); a test serving CKAN on loopback sets its own.
    pub catalogue_http: reqwest::Client,
    /// `None` when no Keycloak realm is configured: login answers 503, every protected
    /// route answers 401. Fail closed.
    pub oidc: Option<Arc<OidcClient>>,
    /// Verifies `Authorization: Bearer` tokens against the realm JWKS. `None` without a realm:
    /// every bearer call answers 401. Fail closed.
    pub bearer: Option<Arc<BearerVerifier>>,
    pub mirror: Arc<Mirror>,
    /// `None` when no forge is configured: every write answers 503. A Portal that cannot
    /// open a merge request must not fall back to a local write (CC-03).
    pub gitea: Option<Arc<GiteaClient>>,
    /// Where application repositories are copied to on GitHub (AP-79). `None` when the
    /// installation keeps them on its forge alone.
    pub github_mirror: Option<Arc<crate::git::github_mirror::GithubMirror>>,
    pub syncer: Option<Arc<Syncer>>,
    /// The `SyncSource` loop (MF-27…MF-32). `None` without a forge, or when the outbound HTTP
    /// client could not be built: the sync routes answer 503 and nothing syncs, rather than a
    /// loop that quietly reaches nothing.
    pub sync: Option<Arc<crate::sync::driver::Driver>>,
    /// The preferences tier (UI-09). `None` without a database: the preferences routes answer
    /// 503 and nothing else notices.
    pub db: Option<sqlx::PgPool>,
    /// The people of the realm, through the admin client (PF-90). `None` without that client:
    /// the people routes then answer 503.
    pub people: Option<Arc<crate::people::People>>,
    /// Builder runs and their event streams (AG-43, AG-45). Always present, durable only when
    /// there is a database; [`AgentStore::is_durable`] is what says which.
    pub agents: Arc<AgentStore>,
    /// The live half of a run's stream: what a connected browser is handed as the events
    /// arrive, while the store is what a reconnecting one replays from (AG-45).
    pub agent_events: Arc<AgentEventHub>,
    /// Drafts shared across browser windows, assistant runs and MCP clients (AG-61, UI-47).
    pub drafts: DraftStore,
    /// Live hub for draft change events (UI-47).
    pub draft_events: DraftHub,
    /// The workspace registry (CC-76): durable with a database, in memory without one.
    pub workspaces: crate::ops::workspaces::WorkspaceStore,
    /// Saved data views of the spaces (API/01 §30).
    pub data_views: crate::ops::data_views::DataViewStore,
    pub pipeline_templates: crate::ops::pipeline_templates::TemplateStore,
    /// The renders of running workspace previews, by branch head (CC-78).
    pub previews: Arc<crate::ops::previews::Renders>,
    /// What is happening in a project (UI-31, OPS-48). Always present, durable only when there
    /// is a database: without one, the Portal shows what happened since it started.
    pub activity: crate::activity::ActivityStore,
    /// The live half of the activity stream: the tail a connected browser follows.
    pub activity_events: crate::activity::ActivityHub,
    /// What authorises a run through the sync webhook route, by source (MF-44). Always present;
    /// empty until the reconciler has resolved a pass, so the door is shut before it is opened.
    pub webhook_secrets: Arc<crate::sync::webhook_secrets::Accepted>,
    /// Each space's personal-data key as the last sync resolved it (DM-121): what a data
    /// subject is hashed with to find a `keyedHash` subject slot. Empty until a sync ran.
    pub personal_keys: Arc<crate::personal_keys::Keys>,
    /// The compiled model of every space that names one: what the reconciler renders the
    /// validation stage from and the rejected route names a rule by (PL-60, PL-61).
    pub model_schemas: Arc<crate::pipeline_validation::ModelSchemas>,
    /// The records each pipeline's stage refused (PL-61): durable with a database, in memory
    /// without one.
    pub rejected: Arc<crate::pipeline_outcomes::RejectedStore>,
    /// Each person's copies of the entities they deleted from a data view (API/01 §31, T-3107).
    pub trash: Arc<crate::entity_trash::TrashStore>,
    /// Comments on a space's entities and the notifications of their mentions (API/01 §35).
    pub comments: Arc<crate::entity_comments::CommentStore>,
    pub feedback: Arc<crate::feedback::FeedbackStore>,
    /// People's connectors and their audit (AG-142, AG-147).
    pub connectors: Arc<crate::connectors::ConnectorStore>,
    /// Portal tabs paired with a connector, which hear its UI actions (T-5967).
    pub paired_tabs: Arc<crate::paired_tabs::PairedTabs>,
    /// Plans waiting for their apply (AG-149, T-5956).
    pub plans: Arc<crate::ops::plans::PlanStore>,
    /// The alerts people chose, their incidents and notices (API/01 §37, T-3261).
    pub alerts: Arc<crate::alerts::AlertStore>,
    /// The health log of every project's resources (T-5067).
    pub health_log: Arc<crate::health_log::HealthStore>,
    /// The weekly link check of each pipeline (T-5133).
    pub link_check: Arc<crate::link_check::LinkStore>,
    /// Calls per Endpoint, surface and day, as the gateways post them (T-5118).
    pub endpoint_usage: Arc<crate::endpoint_usage::UsageStore>,
    /// What `capture` MqttBrokers kept (ADR-N-048 §2.10, T-4880).
    pub mqtt_capture: Arc<crate::mqtt_capture::CaptureStore>,
    /// The live updates of open data views (API/01 §32, T-3105).
    pub live: Arc<crate::live::LiveHub>,
    /// The runner readings behind each pipeline's stats, kept 15 minutes (PL-68).
    pub pipeline_stats: Arc<crate::pipeline_stats::Samples>,
    /// Each pipeline's runs and their log (PL-62), durable with a database.
    pub pipeline_log: Arc<crate::pipeline_log::LogStore>,
    /// The source ledgers and holds of the snapshot pipelines (PL-97, T-4891).
    pub source_ledger: Arc<crate::pipeline_source_deletes::Ledger>,
    /// The last messages through each node of a running stream, in memory (T-3974).
    pub pipeline_samples: Arc<crate::pipeline_samples::SampleStore>,
    /// What the last drift scan found, by project (CC-21). Always present; empty until the
    /// reconciler has run one, which is a different answer from "nothing drifted".
    pub drift: Arc<crate::reconciler::drift::Store>,
    /// What the last data-quality run found, by space (DM-74). Empty until the leader has run
    /// one, which the API answers as "not checked yet".
    pub quality: Arc<crate::quality::Store>,
    /// The gateway's quality counters as this replica read them, for the 24-hour and 7-day
    /// windows of a space's checks on write (DM-94).
    pub quality_checks: Arc<crate::quality_checks::Readings>,
    /// How much each space holds, as the broker counts it (API/01 §29).
    pub space_usage: Arc<crate::space_usage::Counter>,
    /// Moves of spaces between brokers (API/01 §45, PF-115).
    pub space_moves: Arc<crate::space_move::Moves>,
    /// The space surface a resolution writes through (UI-26). `None` without a gateway address
    /// or a realm client: the two buttons answer 503 rather than writing nowhere.
    pub drift_watch: Option<Arc<crate::reconciler::drift::Watch>>,
    /// The broker view writer, which the filter preview compiles a draft with (T-3802).
    pub views: Option<Arc<crate::reconciler::views::ViewSync>>,
    /// The realm's clients and groups this platform did not create, as the last reconcile run
    /// listed them (AP-114, AP-115). Always present; empty until a run has listed the realm.
    pub foreign_names: Arc<crate::reconciler::foreign::ForeignNames>,
    /// Where a workspace Job is written. `None` outside a cluster, exactly like
    /// `app_settings`: a run is then refused rather than scheduled nowhere (AG-33).
    pub kube: Option<Arc<crate::apps::kube::KubeClient>>,
    /// Where a run tests each version before it offers publication (SDK-38). `None` without the
    /// sandbox's settings or outside a cluster: the run says its tests were not run.
    pub app_tests: Option<Arc<crate::agents::sandbox::Sandbox>>,
    /// `sub` → unix second of the last back-channel logout for that user. Sessions issued
    /// at or before the mark are refused.
    /// ponytail: per-replica map; move it to the preferences database when the portal
    /// runs more than one replica (the reconciler is leader-elected, the UI is not).
    revocations: Arc<RwLock<HashMap<String, i64>>>,
    /// What a cookie session keeps on the server, by `sub` and `issued_at` (T-6549): the ID
    /// token (only the `id_token_hint` of its logout) and the group paths. Both grow with the
    /// person's groups, and a browser drops a cookie past 4096 bytes.
    /// ponytail: per-replica and lost on restart. The next request then refreshes the session,
    /// which reads the groups from a fresh ID token, and a logout before that falls back to
    /// `client_id` (Keycloak asks to confirm); move it to the database with `revocations`.
    session_claims: Arc<RwLock<HashMap<(String, i64), SessionClaims>>>,
    /// The calls each key is answering right now (T-5968); a key leaves at zero.
    mcp_in_flight: Arc<Mutex<HashMap<String, u32>>>,
    /// The forge reads of the open changes, per head and base commit (T-6347).
    pub change_reads: crate::api::changes::ChangeReads,
    /// The MCP calls that outlive their request (AG-60): a client starts one, polls it and
    /// reads its result through `tasks/*`.
    pub mcp_tasks: crate::mcp::tasks::McpTasks,
    /// The questions a Yellow or Red MCP call is waiting on (AG-63).
    pub mcp_elicitations: crate::mcp::elicitation::McpElicitations,
}

/// How long a logout mark is kept: a day past the longest a session lives, so no session can
/// outlive the mark that refuses it (PF-11).
const REVOCATION_TTL_SECS: i64 = 48 * 3600;

impl AppState {
    /// Whether this replica runs the work with side effects of a background loop (W-R2POR1-012):
    /// the reconciler's leader, or the one replica there is when nothing elects one.
    pub fn leads(&self) -> bool {
        self.syncer.as_ref().is_none_or(|syncer| syncer.is_leader())
    }

    /// The forge client for `project`'s own files (CC-87): in layout 2 its own repository,
    /// spoken to in render paths (`projects/{project}/…`); the organization repository
    /// otherwise. `None` when no forge is configured.
    pub fn forge_for(&self, project: &str) -> Option<Arc<GiteaClient>> {
        let organization = self.gitea.as_ref()?;
        Some(match self.mirror.repository_of(project) {
            Some(repository) => Arc::new(organization.for_project(repository, project)),
            None => Arc::clone(organization),
        })
    }

    pub fn new(config: Config, oidc: Option<OidcClient>) -> Self {
        let sdm_mirror = config.sdm_mirror_dir.as_deref().and_then(|dir| {
            crate::tools::sdm_mirror::SdmMirror::open(std::path::Path::new(dir))
                .inspect_err(
                    |err| tracing::warn!(%err, "the Smart Data Models mirror did not read"),
                )
                .ok()
                .map(Arc::new)
        });
        let public_base = config.public_base_url.to_string();
        let bearer = config
            .oidc
            .as_ref()
            .map(|o| Arc::new(BearerVerifier::new(&o.issuer, &o.client_id)));
        let draft_events = DraftHub::new();
        let drafts = DraftStore::new(None).with_hub(draft_events.clone());
        let space_usage = Arc::new(
            crate::space_usage::Counter::new(
                config.broker_url.clone(),
                config.broker_admin_token_file.clone(),
            )
            .with_broker_template(config.broker_url_template.clone()),
        );
        let quality_checks = Arc::new(crate::quality_checks::Readings::new(
            config.gateway_url.clone(),
        ));
        let activity_events = crate::activity::ActivityHub::new();
        let activity = crate::activity::ActivityStore::new(None).with_hub(activity_events.clone());
        Self {
            bearer,
            catalogue_http: crate::api::catalogue::CKAN_HTTP.clone(),
            config: Arc::new(config),
            oidc: oidc.map(Arc::new),
            mirror: Arc::new(Mirror::new()),
            gitea: None,
            github_mirror: None,
            syncer: None,
            sync: None,
            db: None,
            agents: Arc::new(AgentStore::new(None)),
            agent_events: Arc::new(AgentEventHub::new()),
            drafts,
            draft_events,
            workspaces: crate::ops::workspaces::WorkspaceStore::new(None),
            data_views: crate::ops::data_views::DataViewStore::new(None),
            pipeline_templates: crate::ops::pipeline_templates::TemplateStore::new(None),
            previews: Arc::default(),
            activity,
            activity_events,
            webhook_secrets: Arc::new(crate::sync::webhook_secrets::Accepted::new()),
            personal_keys: Arc::default(),
            drift: Arc::new(crate::reconciler::drift::Store::default()),
            quality: Arc::new(crate::quality::Store::default()),
            quality_checks,
            space_usage,
            space_moves: Arc::default(),
            model_schemas: Arc::default(),
            rejected: Arc::new(crate::pipeline_outcomes::RejectedStore::new(None)),
            trash: Arc::new(crate::entity_trash::TrashStore::new(None)),
            comments: Arc::new(crate::entity_comments::CommentStore::new(None)),
            feedback: Arc::new(crate::feedback::FeedbackStore::new(None)),
            connectors: Arc::new(crate::connectors::ConnectorStore::new(None)),
            paired_tabs: Arc::default(),
            plans: Arc::default(),
            alerts: Arc::new(crate::alerts::AlertStore::new(None)),
            health_log: Arc::new(crate::health_log::HealthStore::new(None)),
            link_check: Arc::new(crate::link_check::LinkStore::new(None)),
            endpoint_usage: Arc::new(crate::endpoint_usage::UsageStore::new(None)),
            mqtt_capture: Arc::new(crate::mqtt_capture::CaptureStore::new(None)),
            live: Arc::new(crate::live::LiveHub::new(&public_base, None)),
            pipeline_log: Arc::new(crate::pipeline_log::LogStore::new(None)),
            source_ledger: Arc::new(crate::pipeline_source_deletes::Ledger::new(None)),
            pipeline_samples: Arc::default(),
            pipeline_stats: Arc::default(),
            drift_watch: None,
            views: None,
            foreign_names: Arc::default(),
            people: None,
            kube: None,
            app_tests: None,
            revocations: Arc::new(RwLock::new(HashMap::new())),
            session_claims: Arc::default(),
            mcp_in_flight: Arc::default(),
            change_reads: Default::default(),
            mcp_tasks: crate::mcp::tasks::McpTasks::new(),
            mcp_elicitations: crate::mcp::elicitation::McpElicitations::new(),
            wasm_apps: None,
            sdm_mirror,
        }
    }

    /// The apps database and bucket a `wasm` App is published with (AP-149, AP-151).
    pub fn with_wasm_apps(mut self, wasm: Arc<crate::apps::wasm_apps::WasmApps>) -> Self {
        self.wasm_apps = Some(wasm);
        self
    }

    /// A run's test sandbox (SDK-38), as a test gives it one.
    pub fn with_app_tests(mut self, sandbox: Arc<crate::agents::sandbox::Sandbox>) -> Self {
        self.app_tests = Some(sandbox);
        self
    }

    pub fn with_mirror(mut self, mirror: Arc<Mirror>) -> Self {
        self.mirror = mirror;
        self
    }

    pub fn with_gitea(mut self, gitea: Arc<GiteaClient>) -> Self {
        self.gitea = Some(gitea);
        self
    }

    pub fn with_github_mirror(
        mut self,
        mirror: Arc<crate::git::github_mirror::GithubMirror>,
    ) -> Self {
        self.github_mirror = Some(mirror);
        self
    }

    pub fn with_syncer(mut self, syncer: Arc<Syncer>) -> Self {
        self.syncer = Some(syncer);
        self
    }

    pub fn with_db(mut self, db: sqlx::PgPool) -> Self {
        self.attach_stores(&Some(db.clone()));
        self.db = Some(db);
        self
    }

    /// Builds the state for a configuration, discovering the Keycloak realm when one is set and
    /// picking up the forge from the environment. Both failures are fatal at startup: the caller
    /// only prints them, so one boxed error is enough for the two kinds.
    pub async fn from_config(
        config: Config,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let oidc = match config.oidc.as_ref() {
            Some(oidc_config) => {
                Some(OidcClient::discover(oidc_config, &config.redirect_uri()).await?)
            }
            None => None,
        };
        // A half-configured forge is a configuration error, not a reason to run without one:
        // `from_env` answers `Ok(None)` only when all four variables are absent.
        let gitea = GiteaClient::from_env(|key| std::env::var(key).ok())?;
        // The same rule for the GitHub copy of application repositories (AP-79): off when both
        // of its variables are absent, a startup error when only one is.
        let github_mirror =
            crate::git::github_mirror::GithubMirror::from_env(|key| std::env::var(key).ok())?;
        // A configured database that cannot be reached or migrated is fatal, like a half-configured
        // forge: better one clear startup error than a Portal that silently forgets preferences.
        let db = match config.database_url.as_deref() {
            Some(url) => Some(crate::db::connect(url).await?),
            None => None,
        };
        let mut state = Self::new(config, oidc);
        state.attach_stores(&db);
        if let Err(err) = state.quality.load().await {
            tracing::warn!(error = %err, "the kept data-quality reports were not read");
        }
        // The live updates write their subscriptions as the reconciler writes declared ones: through
        // the space surface, as this Portal's own ServiceAccount (API/01 §32).
        if let (Some(base), Some(oidc), Some((id, secret)), Some(domain)) = (
            state.config.gateway_url.clone(),
            state.config.oidc.as_ref(),
            state.config.keycloak_admin.clone(),
            state.config.org_domain.clone(),
        ) {
            state.live = Arc::new(crate::live::LiveHub::new(
                state.config.public_base_url.as_str(),
                Some(Arc::new(
                    crate::reconciler::subscriptions::SubscriptionSync::new(
                        base,
                        &oidc.backchannel(),
                        id,
                        secret,
                        domain,
                    ),
                )),
            ));
        }
        state.db = db;
        // What the process before this one refused stays refused (T-0980).
        state.load_revocations().await;
        state.attach_run_clients();
        // The people of the realm, managed with the admin client that manages its groups (PF-90).
        if let (Some(oidc), Some((id, secret))) = (
            state.config.oidc.as_ref(),
            state.config.keycloak_admin.clone(),
        ) {
            state.people =
                crate::people::People::new(&oidc.backchannel(), id, secret).map(Arc::new);
        }
        state.attach_wasm_apps();
        // Warm the key cache so the first bearer call does not pay for the fetch; a realm that
        // is down at startup only costs a warning, the next unknown `kid` fetches again.
        if let Some(bearer) = state.bearer.as_ref() {
            if let Err(err) = bearer.refresh().await {
                tracing::warn!(error = %err, "JWKS not loaded at startup");
            }
        }
        if let Some(client) = gitea {
            let client = Arc::new(client);
            let mut syncer = Syncer::new(Arc::clone(&client), Arc::clone(&state.mirror))
                .with_webhook_secrets(Arc::clone(&state.webhook_secrets))
                .with_personal_keys(
                    Arc::clone(&state.personal_keys),
                    state.config.gateway_namespace.clone(),
                )
                .with_activity(state.activity.clone())
                .with_pipeline_outcomes(
                    Arc::clone(&state.rejected),
                    Arc::clone(&state.pipeline_log),
                )
                .with_apps_cache_dir(state.config.apps_cache_dir.clone())
                .with_apps_dir(state.config.apps_dir.clone())
                // The alerts people chose (T-3261): from the statuses every run of the leader sets.
                .with_alerts(Arc::new(
                    crate::alerts::Evaluator::new(
                        Arc::clone(&state.mirror),
                        Arc::clone(&state.quality),
                        Arc::clone(&state.alerts),
                    )
                    .with_source_holds(Arc::clone(&state.source_ledger))
                    .with_run_health(Arc::clone(&state.pipeline_log))
                    .with_agent_runs(Arc::clone(&state.agents))
                    .with_health_log(
                        Arc::clone(&state.health_log),
                        Arc::new(crate::health_probe::Probes::new(
                            state
                                .config
                                .gateway_url
                                .as_deref()
                                .and_then(|g| url::Url::parse(g).ok()),
                            Some(Arc::clone(&state.space_usage)),
                        )),
                    )
                    .with_link_check(
                        state
                            .config
                            .gateway_url
                            .as_deref()
                            .and_then(|g| url::Url::parse(g).ok())
                            .map(|gateway| {
                                Arc::new(crate::link_check::Checker::new(
                                    Arc::clone(&state.mirror),
                                    Arc::clone(&state.link_check),
                                    gateway,
                                ))
                            }),
                    ),
                ));
            syncer = state.wire_cluster_writers(syncer);
            syncer = state.wire_database_steps(syncer);
            syncer = state.wire_app_deployment(syncer);
            syncer = state.wire_realm_clients(syncer);
            syncer = state.wire_service_accounts(syncer);
            syncer = state.wire_streams(syncer);
            syncer = state.wire_space_writers(syncer);
            // The `SyncSource` loop needs the forge and a way out to the origins. Without the
            // second there is no loop at all: a driver that cannot fetch would report every
            // source as failing every minute (MF-27).
            match crate::sync::remote::HttpRemote::new() {
                Ok(remote) => {
                    let states = Arc::new(crate::sync::state::States::new(state.db.clone()));
                    if !states.is_durable() {
                        tracing::info!(
                            "no database: sync sources remember their runs only until this \
                             process ends"
                        );
                    }
                    state.sync = Some(Arc::new(crate::sync::driver::Driver::new(
                        Arc::clone(&client),
                        states,
                        Arc::new(remote),
                    )));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "no HTTP client for sync origins; sync sources do not run")
                }
            }
            state.gitea = Some(client);
            state.syncer = Some(Arc::new(syncer));
        }
        // A copy has nothing to copy without the forge it mirrors.
        match (github_mirror, state.gitea.is_some()) {
            (Some(mirror), true) => state.github_mirror = Some(Arc::new(mirror)),
            (Some(_), false) => tracing::warn!(
                "JC_APP_MIRROR_GITHUB_OWNER is set but no forge is configured: application \
                 repositories are not copied to GitHub"
            ),
            (None, _) => {}
        }
        // The 24-hour and 7-day windows need a reading an hour, page or no page (DM-94).
        state.quality_checks.start_hourly();
        Ok(state)
    }

    /// The writers into the cluster: the artifact store's root client, the WASM shards, the MQTT
    /// brokers, the credential Secrets and the pipelines' secret resolver.
    fn wire_cluster_writers(&self, mut syncer: Syncer) -> Syncer {
        // The root credential of the artifact store reaches this one object and no other,
        // and no workload gets it: every organization is served a derived, scoped pair
        // instead (PF-32, ADR-N-015).
        if let Some(settings) = self.config.artifact_store.clone() {
            match crate::artifact_store::Client::new(settings) {
                Ok(store) => syncer = syncer.with_artifact_store(Arc::new(store)),
                Err(err) => {
                    tracing::warn!(error = %err, "the artifact store endpoint is unusable, so no organization credential is issued")
                }
            }
        }
        // Each WASM shard's placement and store key, in the host's namespace (AP-157, AP-158).
        // Without that namespace there is nowhere a shard would read them, so none is written.
        if let Some(apps_db) = self.config.apps_db.clone() {
            match (apps_db.host_namespace.clone(), crate::apps::kube::KubeClient::in_cluster()) {
                (Some(namespace), Ok(Some(kube))) => {
                    syncer = syncer.with_wasm_shards(Arc::new(
                        crate::reconciler::wasm_shards::WasmShards::new(
                            kube,
                            namespace,
                            apps_db.shards,
                            apps_db.bucket.clone(),
                        ),
                    ));
                }
                (None, _) => tracing::warn!(
                    "JC_PORTAL_WASM_HOST_NAMESPACE is not set: no WASM shard gets its placement or its store key"
                ),
                (_, Err(err)) => {
                    tracing::warn!(error = %err, "the ServiceAccount mount is unreadable, so no WASM shard gets its placement")
                }
                (_, Ok(None)) => tracing::info!("no cluster: no WASM shard placement is written"),
            }
        }
        // Each MqttBroker's release, in the brokers' namespace, from the deployment's template
        // (ADR-N-048, T-3994).
        match (
            self.config.mqtt_namespace.clone(),
            crate::apps::kube::KubeClient::in_cluster(),
        ) {
            (Some(namespace), Ok(Some(kube))) => {
                syncer = syncer.with_mqtt_capture(Arc::new(crate::mqtt_capture::Capture::new(
                    Arc::clone(&self.mqtt_capture),
                    namespace.clone(),
                )));
                syncer = syncer.with_mqtt_brokers(Arc::new(
                    crate::reconciler::mqtt_brokers::MqttBrokers::new(kube, namespace),
                ));
            }
            (None, _) => {
                tracing::info!("JC_PORTAL_MQTT_NAMESPACE is not set: no MqttBroker gets a release")
            }
            (_, Err(err)) => {
                tracing::warn!(error = %err, "the ServiceAccount mount is unreadable, so no MqttBroker gets a release")
            }
            (_, Ok(None)) => tracing::info!("no cluster: no MqttBroker release is applied"),
        }
        // The Secrets this reconciler writes into its own namespace: an organization's
        // artifact-store reader (T-0925) and the pipeline runner's environment (T-0927).
        // Outside a cluster there is nowhere to write one, which is not an error.
        match (
            crate::apps::kube::KubeClient::in_cluster(),
            crate::apps::kube::KubeClient::own_namespace(),
        ) {
            (Ok(Some(kube)), Some(namespace)) => {
                syncer = syncer.with_credential_secrets(Arc::new(kube), namespace);
            }
            (Err(err), _) => {
                tracing::warn!(error = %err, "the ServiceAccount mount is unreadable, so no credential is handed over")
            }
            _ => tracing::info!("no cluster: credentials are resolved and handed to nobody"),
        }
        // What resolves a pipeline's `secretRef`s (T-0927, PL-15). Without it a pipeline
        // that declares one is not deployed, with the reason on the Pipeline; the values
        // reach the runner through the same Secret writer the credentials above use.
        if let Some(backend) = self.config.pipeline_secrets.clone() {
            syncer = syncer.with_pipeline_secrets(crate::pipeline_secrets::Resolver::new(backend));
        }
        // A pipeline keeps only its proposer's and approver's reach (PL-95), the realm asked
        // for their groups when the Portal holds its admin client.
        syncer = syncer.with_author_rights(crate::reconciler::pipeline_authors::AuthorRights::new(
            self.config.bootstrap_admins.clone(),
            self.people.clone(),
        ));
        syncer
    }

    /// The steps that need the database: removals, leadership, pipeline status, domain checks.
    fn wire_database_steps(&self, mut syncer: Syncer) -> Syncer {
        // A person's removal finishes once its Change is merged (PF-93): the pending ones are
        // in the database, so the step runs only with one and with the realm's admin client.
        if let (Some(people), Some(pool)) = (self.people.clone(), self.db.clone()) {
            syncer = syncer.with_people(people, pool);
        }
        // With a database the replicas elect one reconciler; without one there is nothing
        // to elect with, and a Portal that runs alone reconciles alone (T-0191, CC-03).
        if let Some(pool) = self.db.as_ref() {
            syncer = syncer
                .with_leadership(Arc::new(Leadership::reconciler(pool.clone())))
                .with_pipeline_status(Arc::new(crate::pipeline_status::Store::new(pool.clone())));
            // PF-41: the challenge lives in the database, so the check runs only with one.
            let host = self.config.public_base_url.host_str().map(str::to_owned);
            match (crate::domain_verification::NetLookup::new(), host) {
                (Ok(lookup), Some(host)) => {
                    syncer = syncer.with_domain_verification(Arc::new(
                        crate::domain_verification::Verifier::new(pool.clone(), lookup, host),
                    ));
                }
                (Err(err), _) => {
                    tracing::warn!(error = %err, "no domain is verified: the resolver did not start")
                }
                (_, None) => {
                    tracing::warn!("no domain is verified: the public URL names no host")
                }
            }
        }
        syncer
    }

    /// The app converger and the edge file, when there are app settings and a cluster.
    fn wire_app_deployment(&self, mut syncer: Syncer) -> Syncer {
        // Applying an app's objects needs two halves: a cluster to write into and the
        // settings that say where. Either missing leaves the reconciler reading apps and
        // deploying nothing, which is what a Portal on a laptop does (T-0411, AP-18). No
        // realm is needed: the login front is the edge's one client (AP-27, ADR-N-019).
        match (
            self.config.app_settings.clone(),
            crate::apps::kube::KubeClient::in_cluster(),
        ) {
            (Some(settings), Ok(Some(kube))) => {
                // The edge file has its own client: the converger owns the other one.
                match crate::apps::kube::KubeClient::in_cluster() {
                    Ok(Some(edge_kube)) => {
                        let namespace = settings.apisix_namespace.clone();
                        match crate::apps::kube::KubeClient::in_cluster() {
                            Ok(Some(hosts_kube)) => {
                                syncer = syncer.with_app_hosts(Arc::new(
                                    crate::reconciler::app_hosts::AppHosts::new(
                                        hosts_kube,
                                        namespace.clone(),
                                    ),
                                ));
                            }
                            _ => tracing::warn!(
                                "no third API client: no App host gets a certificate"
                            ),
                        }
                        syncer = syncer.with_edge_file(
                            Arc::new(
                                crate::reconciler::edge_file::EdgeFile::new(edge_kube, namespace)
                                    .with_bounds(self.config.organization_bounds.clone())
                                    .with_wasm_host(
                                        self.config
                                            .apps_db
                                            .as_ref()
                                            .and_then(|db| db.host_namespace.clone()),
                                    ),
                            ),
                            settings.clone(),
                        );
                    }
                    _ => tracing::warn!("no second API client: the edge serves helm's base alone"),
                }
                syncer = syncer.with_converger(Arc::new(
                    crate::apps::converge::Converger::new(kube, settings)
                        .with_wasm_apps(self.wasm_apps.clone()),
                ));
            }
            (_, Err(err)) => {
                tracing::warn!(error = %err, "the ServiceAccount mount is unreadable, so no app is deployed")
            }
            _ => tracing::info!("no app settings or no cluster: apps are read, not deployed"),
        }
        syncer
    }

    /// The realm writers: groups, App clients, workload clients, hub scopes, MQTT and MCP clients.
    fn wire_realm_clients(&self, mut syncer: Syncer) -> Syncer {
        // The realm's managed groups (PF-63). Without an admin client the manifests are
        // still read and served; nothing in the realm is written.
        match (
            self.config.oidc.as_ref(),
            self.config.keycloak_admin.clone(),
        ) {
            (Some(oidc), Some((id, secret))) => {
                match crate::reconciler::groups::GroupSync::new(&oidc.backchannel(), id, secret) {
                    Some(groups) => {
                        syncer = syncer.with_groups(Arc::new(
                            groups.with_foreign(Arc::clone(&self.foreign_names)),
                        ))
                    }
                    None => tracing::warn!(
                        "the issuer is not a realm URL, so no Keycloak group is managed"
                    ),
                }
            }
            _ => tracing::info!(
                "no Keycloak admin client: Group manifests are read, the realm is not written"
            ),
        }
        // Every published App's own client (ADR-N-030, AP-111). It is written as the Portal
        // client's own service account, the one identity the realm gives `manage-clients`.
        // Who holds an App role is looked up and mapped with the group credential above,
        // which already writes memberships; the login client only reads the holders (T-3022).
        match (
            self.config.oidc.as_ref(),
            self.config
                .app_settings
                .as_ref()
                .map(|settings| settings.apex.clone()),
        ) {
            (Some(oidc), Some(host)) => {
                match crate::reconciler::app_clients::AppClientSync::new(
                    &oidc.backchannel(),
                    oidc.client_id.clone(),
                    oidc.auth.clone(),
                    host,
                ) {
                    Some(mut clients) => {
                        if let Some((id, secret)) = self.config.keycloak_admin.clone() {
                            clients = clients.with_members(id, secret);
                        }
                        syncer = syncer.with_app_clients(Arc::new(
                            clients.with_foreign(Arc::clone(&self.foreign_names)),
                        ))
                    }
                    None => {
                        tracing::warn!("the issuer is not a realm URL, so no App client is managed")
                    }
                }
            }
            _ => tracing::info!("no login client or no apps host: no App client is managed"),
        }
        // Every ServiceAccount bound to a workload gets its federated client (PF-47), written
        // by the same identity as the App clients.
        if let Some(oidc) = self.config.oidc.as_ref() {
            match crate::reconciler::workload_clients::WorkloadClientSync::new(
                &oidc.backchannel(),
                oidc.client_id.clone(),
                oidc.auth.clone(),
            ) {
                Some(clients) => {
                    let clients = match self.config.pipeline_namespace.clone() {
                        Some(namespace) => clients.with_pipelines(namespace),
                        None => clients,
                    };
                    let clients = match self.config.app_identity_namespace.clone() {
                        Some(namespace) => clients.with_app_jobs(namespace),
                        None => clients,
                    };
                    // A removed account's tokens end with it (T-5568).
                    let state = self.clone();
                    let clients = clients.with_revoke(Arc::new(move |subject: String| {
                        let state = state.clone();
                        Box::pin(async move {
                            state
                                .revoke_subject(&subject, crate::auth::session::now_unix())
                                .await
                        })
                    }));
                    syncer = syncer.with_workload_clients(Arc::new(clients))
                }
                None => tracing::warn!(
                    "the issuer is not a realm URL, so no workload client is managed"
                ),
            }
            // The MCP hub's `endpoint:{slug}` scopes (T-2490), by the same identity.
            if let Some(scopes) = crate::reconciler::hub_scopes::HubScopeSync::new(
                &oidc.backchannel(),
                oidc.client_id.clone(),
                oidc.auth.clone(),
            ) {
                syncer = syncer.with_hub_scopes(Arc::new(scopes));
            }
            // Each MQTT broker's bearer-only client, its devices' audience (T-3994).
            if let Some(clients) = crate::reconciler::mqtt_brokers::MqttClientSync::new(
                &oidc.backchannel(),
                oidc.client_id.clone(),
                oidc.auth.clone(),
            ) {
                syncer = syncer.with_mqtt_clients(Arc::new(clients));
            }
            // Each named MCP server's sign-in client (EP-96, T-3156), by the same identity.
            if let Some(clients) = crate::reconciler::mcp_clients::McpClientSync::new(
                &oidc.backchannel(),
                oidc.client_id.clone(),
                oidc.auth.clone(),
            ) {
                syncer = syncer.with_mcp_clients(Arc::new(clients));
            }
        }
        syncer
    }

    /// The Kubernetes ServiceAccounts of each Pipeline and each App's job.
    fn wire_service_accounts(&self, mut syncer: Syncer) -> Syncer {
        // Each Pipeline's own Kubernetes ServiceAccount (PL-19, T-1508), by the Portal's
        // in-cluster identity, which the deployment lets write that one namespace.
        if let Some(namespace) = self.config.pipeline_namespace.clone() {
            match crate::apps::kube::KubeClient::in_cluster() {
                Ok(Some(kube)) => {
                    syncer = syncer.with_pipeline_service_accounts(Arc::new(kube), namespace)
                }
                Ok(None) => tracing::warn!(
                    "no ServiceAccount mount: the pipelines' Kubernetes accounts are not made"
                ),
                Err(err) => tracing::warn!(
                    error = %err,
                    "the ServiceAccount mount is unreadable, so the pipelines' Kubernetes accounts are not made"
                ),
            }
        }
        // Each App's job ServiceAccount (AP-159, T-3539), by the same in-cluster identity,
        // which the deployment lets write the identities namespace.
        match self.config.app_identity_namespace.clone() {
            Some(namespace) => match crate::apps::kube::KubeClient::in_cluster() {
                Ok(Some(kube)) => {
                    syncer = syncer.with_app_job_service_accounts(Arc::new(kube), namespace)
                }
                Ok(None) => {
                    tracing::warn!("no ServiceAccount mount: the Apps' job accounts are not made")
                }
                Err(err) => tracing::warn!(
                    error = %err,
                    "the ServiceAccount mount is unreadable, so the Apps' job accounts are not made"
                ),
            },
            None => {
                tracing::info!("no JC_PORTAL_APP_IDENTITY_NAMESPACE: no App job gets a principal")
            }
        }
        syncer
    }

    /// The stream deployer, with its queue started, when a pipeline runner is configured.
    fn wire_streams(&self, mut syncer: Syncer) -> Syncer {
        if let Some(url) = self.config.pipeline_runner_url.clone() {
            let (batch, short) = self.config.pipelines_concurrency;
            // A snapshot pipeline judges its source deletes when its run ends (T-4891).
            let (ends, mut ended) = tokio::sync::mpsc::unbounded_channel();
            let judge = self.clone();
            tokio::spawn(async move {
                while let Some(end) = ended.recv().await {
                    crate::pipeline_source_deletes::run_ended(&judge, end).await;
                }
            });
            // An events pipeline without runs deletes what its source marked every sweep (T-5360).
            let sweeper = self.clone();
            tokio::spawn(async move {
                let mut every = tokio::time::interval(crate::pipeline_source_deletes::SWEEP);
                loop {
                    every.tick().await;
                    crate::pipeline_source_deletes::sweep_marks(&sweeper).await;
                }
            });
            let deployer = StreamDeployer::new(url)
                .with_pipeline_identity(self.config.pipeline_identity)
                .with_concurrency(batch, short)
                .with_run_ends(ends);
            // A refused record reaches the Portal on the listener the test harness reaches
            // (PL-61); without that address the stream writes as it did, unvalidated.
            let deployer = match self.config.pipeline_test_capture_url.clone() {
                Some(internal) => {
                    deployer.with_validation(internal, Arc::clone(&self.model_schemas))
                }
                None => {
                    tracing::warn!("no internal listener address for the runner: pipelines are not validated before they write");
                    deployer
                }
            };
            let deployer = Arc::new(deployer);
            // The run-and-delete pipelines run from the queue (PL-78).
            Arc::clone(&deployer).spawn_queue();
            syncer = syncer.with_streams(deployer);
        } else {
            tracing::info!("no pipeline runner: DataSource pipelines stay Pending");
        }
        syncer
    }

    /// The writers into the spaces: subscriptions, the CKAN catalogue, registrations, views and drift.
    fn wire_space_writers(&mut self, mut syncer: Syncer) -> Syncer {
        // A declared Subscription reaches its space through the space surface, as this
        // Portal's own service account (T-0931, CC-72). Without the address, the realm
        // client or the organization's domain there is no write to make: the manifests are
        // read and served, and no broker is touched.
        match (
            self.config.gateway_url.clone(),
            self.config.oidc.as_ref(),
            self.config.keycloak_admin.clone(),
            self.config.org_domain.clone(),
        ) {
            (Some(base), Some(oidc), Some((id, secret)), Some(domain)) => {
                syncer = syncer.with_subscriptions(Arc::new(
                    crate::reconciler::subscriptions::SubscriptionSync::new(
                        base,
                        &oidc.backchannel(),
                        id,
                        secret,
                        domain,
                    ),
                ));
            }
            _ => tracing::info!(
                "no gateway address, realm client or organization domain: Subscription \
                 manifests are read, no broker is written"
            ),
        }
        // An Endpoint that declares `spec.publish.ckan` becomes a dataset in its catalogue on
        // every run, and one that stopped declaring it loses the dataset (EP-62, T-2405). The
        // host is the data host the endpoints answer on, read there directly: through the
        // Portal host each read was a 308 and a second edge request (T-4717); the token comes from the instance's `apiTokenRef`, through the repository's encrypted
        // secrets when this Portal holds the age identity, and otherwise from the variable the
        // reference itself names.
        {
            let age_key_file = match self.config.pipeline_secrets.as_ref() {
                Some(crate::pipeline_secrets::Backend::Sops { age_key_file }) => {
                    Some(age_key_file.clone())
                }
                _ => None,
            };
            let mut catalogue =
                crate::reconciler::ckan::CkanSync::new(catalogue_host(&self.config))
                    .with_age_key_file(age_key_file);
            let branding = crate::branding::Branding::load(self.config.branding_file.as_deref());
            if !branding.organisation.trim().is_empty() {
                catalogue = catalogue.titled(branding.organisation);
            }
            if !branding.documentation_base_url.is_empty() {
                catalogue = catalogue.documented_at(branding.documentation_base_url);
            }
            // The deployment's own catalogue is called on its Service, not through the public
            // edge whose limit the visitors share (T-5314).
            if let Some((public, api)) = &self.config.ckan_api_route {
                catalogue = catalogue.reaching(public.as_str(), api.as_str());
            }
            // Each dataset carries the pass rate and the freshness of its space (DM-95).
            catalogue =
                catalogue.with_quality(Arc::clone(&self.quality), Arc::clone(&self.quality_checks));
            syncer = syncer.with_ckan(Arc::new(catalogue));
        }

        // A declared ContextSourceRegistration is written straight at the broker, in the
        // tenant of its hub space (T-0345, SP-08): it is a control-plane act with no
        // gateway operation behind it, sent with the broker's admin token (T-5032). Without the broker's
        // address the manifests are read and served and no hub is federated.
        match self.config.broker_url.clone() {
            Some(broker) => {
                syncer = syncer.with_registrations(Arc::new(
                    crate::reconciler::registrations::RegistrationSync::new(broker)
                        .with_broker_template(self.config.broker_url_template.clone())
                        .with_gateway(self.config.gateway_url.clone())
                        .with_admin_token(self.config.broker_admin_token_file.clone()),
                ));
            }
            None => tracing::info!(
                "no broker address: ContextSourceRegistration manifests are read, no hub is \
                 federated"
            ),
        }
        // Every Endpoint's and Context Space's view and every space's rule document, written
        // into the broker with its admin token (T-3640). Without both the Endpoints stay on
        // the gateway.
        if let (Some(broker), Some(token_file)) = (
            self.config.broker_url.clone(),
            self.config.broker_admin_token_file.clone(),
        ) {
            let mut views = crate::reconciler::views::ViewSync::new(broker, token_file)
                .with_broker_template(self.config.broker_url_template.clone());
            if let Some(url) = self.config.model_tools_url.clone() {
                views = views.with_model_tools(url);
            }
            let views = Arc::new(views);
            self.views = Some(Arc::clone(&views));
            syncer = syncer.with_views(views);
        }
        // Drift (CC-21, UI-25, UI-26): the seed entities a space declares, against what it
        // holds. Configuration cannot drift under T-0421's option B — every component reads
        // it from the repository — so this is the whole of what drift means here. Without
        // the space surface or the realm client there is nothing to compare against, and
        // the API answers "never scanned" rather than "nothing drifted".
        match (
            self.config.gateway_url.clone(),
            self.config.oidc.as_ref(),
            self.config.keycloak_admin.clone(),
        ) {
            (Some(base), Some(oidc), Some((id, secret))) => {
                let watch = Arc::new(crate::reconciler::drift::Watch::new(
                    base,
                    &oidc.backchannel(),
                    id,
                    secret,
                ));
                // One watch, two readers: the reconciler scans with it and a resolution
                // writes through it, so the buttons cannot reach a surface the scan did not.
                self.drift_watch = Some(Arc::clone(&watch));
                // The daily data-quality run reads through the same client (DM-74).
                syncer = syncer.with_quality(Arc::new(crate::quality::Scanner {
                    watch: Arc::clone(&watch),
                    schemas: Arc::clone(&self.model_schemas),
                    mirror: Arc::clone(&self.mirror),
                    org_domain: self.config.org_domain.clone().unwrap_or_default(),
                    store: Arc::clone(&self.quality),
                }));
                // The NGSI-LD catalogue writes through the same client (T-4500).
                syncer =
                    syncer.with_catalog(Arc::new(crate::reconciler::catalog::CatalogSync::new(
                        Arc::clone(&watch),
                        crate::api::catalogue::ckan_client(
                            crate::reconciler::catalog::CKAN_TIMEOUT,
                        ),
                    )));
                syncer = syncer.with_drift(watch, Arc::clone(&self.drift));
            }
            _ => tracing::info!(
                "no gateway address or realm client: seed entities are never compared, so \
                 the drift surface answers that no scan has run"
            ),
        }
        syncer
    }

    /// The stores that keep their rows in the database when there is one.
    fn attach_stores(&mut self, db: &Option<sqlx::PgPool>) {
        self.agents = Arc::new(AgentStore::new(db.clone()));
        if !self.agents.is_durable() {
            tracing::info!(
                "no database: a builder run and its stream live only until this process ends"
            );
        }
        self.drafts = DraftStore::new(db.clone()).with_hub(self.draft_events.clone());
        if let Some(db) = &db {
            self.draft_events.connect(db.clone());
        }
        self.workspaces = crate::ops::workspaces::WorkspaceStore::new(db.clone());
        self.data_views = crate::ops::data_views::DataViewStore::new(db.clone());
        self.pipeline_templates = crate::ops::pipeline_templates::TemplateStore::new(db.clone());
        self.activity =
            crate::activity::ActivityStore::new(db.clone()).with_hub(self.activity_events.clone());
        self.rejected = Arc::new(crate::pipeline_outcomes::RejectedStore::new(db.clone()));
        self.trash = Arc::new(crate::entity_trash::TrashStore::new(db.clone()));
        self.comments = Arc::new(crate::entity_comments::CommentStore::new(db.clone()));
        self.feedback = Arc::new(crate::feedback::FeedbackStore::new(db.clone()));
        self.connectors = Arc::new(crate::connectors::ConnectorStore::new(db.clone()));
        self.alerts = Arc::new(crate::alerts::AlertStore::new(db.clone()));
        self.health_log = Arc::new(crate::health_log::HealthStore::new(db.clone()));
        self.link_check = Arc::new(crate::link_check::LinkStore::new(db.clone()));
        self.endpoint_usage = Arc::new(crate::endpoint_usage::UsageStore::new(db.clone()));
        self.mqtt_capture = Arc::new(crate::mqtt_capture::CaptureStore::new(db.clone()));
        self.quality = Arc::new(crate::quality::Store::new(db.clone()));
        self.pipeline_log = Arc::new(crate::pipeline_log::LogStore::new(db.clone()));
        self.source_ledger = Arc::new(crate::pipeline_source_deletes::Ledger::new(db.clone()));
    }

    /// The cluster clients a builder run and its test sandbox are scheduled with.
    fn attach_run_clients(&mut self) {
        // A builder run is scheduled into the cluster this Portal runs in (AG-33). The client is
        // the same in-cluster one the app converger uses; outside a cluster it stays `None` and
        // a run is refused rather than recorded with no pod behind it.
        if self.config.agent_settings.is_some() {
            match crate::apps::kube::KubeClient::in_cluster() {
                Ok(Some(kube)) => self.kube = Some(Arc::new(kube)),
                Ok(None) => tracing::info!(
                    "no ServiceAccount mount: a builder run is driven by its caller, not scheduled"
                ),
                Err(err) => tracing::warn!(
                    error = %err,
                    "the ServiceAccount mount is unreadable, so no builder workspace is scheduled"
                ),
            }
        }
        // The run's test sandbox (SDK-38): its own client, in the namespace that fences it.
        if let Some(settings) = self.config.app_tests.clone() {
            match crate::apps::kube::KubeClient::in_cluster() {
                Ok(Some(kube)) => {
                    self.app_tests = Some(Arc::new(crate::agents::sandbox::Sandbox::new(
                        kube, settings,
                    )))
                }
                Ok(None) => tracing::info!(
                    "no ServiceAccount mount: a run's tests are left to the build lane"
                ),
                Err(err) => tracing::warn!(
                    error = %err,
                    "the ServiceAccount mount is unreadable, so a run's tests are left to the build lane"
                ),
            }
        }
    }

    /// The wasm Apps' database and bucket, both or neither.
    fn attach_wasm_apps(&mut self) {
        // The wasm Apps' database and bucket (AP-149, AP-151): both or neither, the pool connects
        // on first use and jc_apps's schema is applied in the background.
        if let (Some(apps_db), Some(store)) = (
            self.config.apps_db.clone(),
            self.config.artifact_store.clone(),
        ) {
            match (
                apps_db.url.parse::<sqlx::postgres::PgConnectOptions>(),
                crate::artifact_store::Client::new(store),
            ) {
                (Ok(connect), Ok(store)) => {
                    let admin = sqlx::postgres::PgPoolOptions::new()
                        .max_connections(4)
                        .connect_lazy_with(connect.clone());
                    let db = crate::apps::apps_db::AppsDb {
                        admin,
                        connect,
                        shards: apps_db.shards,
                    };
                    let wasm = Arc::new(crate::apps::wasm_apps::WasmApps {
                        db,
                        store: Arc::new(store),
                        bucket: apps_db.bucket,
                    });
                    let boot = Arc::clone(&wasm);
                    tokio::spawn(async move {
                        if let Err(err) = boot.db.bootstrap().await {
                            tracing::error!(error = %err, "the apps database's jc_apps schema did not apply");
                        }
                    });
                    self.wasm_apps = Some(wasm);
                }
                (Err(err), _) => {
                    tracing::error!(error = %err, "JC_PORTAL_APPS_DB_URL is no PostgreSQL URL, so no wasm App is published")
                }
                (_, Err(err)) => {
                    tracing::error!(error = %err, "the artifact store is unusable, so no wasm App is published")
                }
            }
        }
    }

    /// Marks every session of a subject as logged out (OIDC back-channel logout).
    ///
    /// The mark is written to the database as well as to this process, because a restart used
    /// to clear every mark and re-accept sessions somebody had logged out for as long as their
    /// cookie lived (T-0980). Without a database the mark is this process's alone, which is what
    /// a Portal without one can honestly promise.
    pub async fn revoke_subject(&self, subject: &str, at: i64) {
        if let Ok(mut marks) = self.revocations.write() {
            let mark = marks.entry(subject.to_string()).or_insert(at);
            *mark = (*mark).max(at);
        }
        let Some(db) = self.db.as_ref() else {
            return;
        };
        // A mark outlives every session it could refuse: `REVOCATION_TTL` past the mark itself.
        if let Err(error) = sqlx::query(
            "INSERT INTO revocations (subject, revoked_at, expires_at) VALUES ($1, $2, $3) \
             ON CONFLICT (subject) DO UPDATE SET \
               revoked_at = GREATEST(revocations.revoked_at, EXCLUDED.revoked_at), \
               expires_at = GREATEST(revocations.expires_at, EXCLUDED.expires_at)",
        )
        .bind(subject)
        .bind(at)
        .bind(
            time::OffsetDateTime::from_unix_timestamp(at + REVOCATION_TTL_SECS)
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc()),
        )
        .execute(db)
        .await
        {
            // The mark holds in this process either way; saying so is what a restart needs.
            tracing::error!(%error, "a logout mark was not persisted and a restart will lose it");
        }
    }

    /// Reads the marks a previous process wrote, and drops the ones no session can outlive.
    ///
    /// ponytail: one read at startup, because one replica serves the UI. A second replica would
    /// also have to see a mark the first one wrote, which is a read on the session path or a
    /// notification — neither is worth its cost while the Deployment is one pod.
    pub async fn load_revocations(&self) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        if let Err(error) = sqlx::query("DELETE FROM revocations WHERE expires_at < now()")
            .execute(db)
            .await
        {
            tracing::warn!(%error, "old logout marks were not trimmed");
        }
        match sqlx::query_as::<_, (String, i64)>(
            "SELECT subject, revoked_at FROM revocations WHERE expires_at >= now()",
        )
        .fetch_all(db)
        .await
        {
            Ok(rows) => {
                if let Ok(mut marks) = self.revocations.write() {
                    for (subject, at) in rows {
                        let mark = marks.entry(subject).or_insert(at);
                        *mark = (*mark).max(at);
                    }
                    tracing::info!(marks = marks.len(), "logout marks read back");
                }
            }
            Err(error) => tracing::error!(
                %error,
                "logout marks could not be read: a session logged out before this start is \
                 accepted again until its cookie expires"
            ),
        }
    }

    /// Holds one call in every key of `slots` (key, calls at once) until the guard drops, or
    /// refuses with the index of the first full one (T-5968).
    pub fn mcp_enter(&self, slots: &[(&str, u32)]) -> Result<InFlight, usize> {
        let Ok(mut held) = self.mcp_in_flight.lock() else {
            return Err(0);
        };
        if let Some(full) = slots
            .iter()
            .position(|(key, max)| held.get(*key).copied().unwrap_or(0) >= *max)
        {
            return Err(full);
        }
        for (key, _) in slots {
            *held.entry((*key).to_owned()).or_default() += 1;
        }
        Ok(InFlight {
            held: Arc::clone(&self.mcp_in_flight),
            keys: slots.iter().map(|(key, _)| (*key).to_owned()).collect(),
        })
    }

    /// The marks a token issued before them could still be valid against at `now`, by subject:
    /// what the gateway refuses on its own doors (T-5568).
    pub fn revocations(&self, now: i64) -> Vec<(String, i64)> {
        let mut marks: Vec<(String, i64)> = self
            .revocations
            .read()
            .map(|marks| {
                marks
                    .iter()
                    .filter(|(_, at)| at.saturating_add(REVOCATION_TTL_SECS) >= now)
                    .map(|(subject, at)| (subject.clone(), *at))
                    .collect()
            })
            .unwrap_or_default();
        marks.sort();
        marks
    }

    /// Keeps a cookie session's ID token and groups (T-6549) and drops every ended session's.
    /// A refresh that brought no ID token keeps the one the session had.
    pub fn keep_session_claims(&self, session: &Session) {
        let now = crate::auth::session::now_unix();
        let key = (session.identity.subject.clone(), session.issued_at);
        if let Ok(mut kept) = self.session_claims.write() {
            kept.retain(|_, claims| claims.expires_at > now);
            let id_token = match (session.id_token.is_empty(), kept.get(&key)) {
                (true, Some(before)) => before.id_token.clone(),
                _ => session.id_token.clone(),
            };
            kept.insert(
                key,
                SessionClaims {
                    expires_at: session.expires_at,
                    id_token,
                    groups: session.identity.groups.clone(),
                },
            );
        }
    }

    /// The groups [`Self::keep_session_claims`] kept for this session; `None` when this process
    /// holds none (a restart, another replica).
    pub fn session_groups(&self, session: &Session) -> Option<Vec<String>> {
        self.session_claims
            .read()
            .ok()?
            .get(&(session.identity.subject.clone(), session.issued_at))
            .map(|claims| claims.groups.clone())
    }

    /// The ID token kept for this session, removed with its groups: a logout ends the session.
    pub fn take_id_token(&self, session: &Session) -> Option<String> {
        self.session_claims
            .write()
            .ok()?
            .remove(&(session.identity.subject.clone(), session.issued_at))
            .map(|claims| claims.id_token)
            .filter(|token| !token.is_empty())
    }

    pub fn is_revoked(&self, session: &Session) -> bool {
        self.revoked_since(&session.identity.subject, session.issued_at)
    }

    /// Whether `subject` was logged out at or after `at` (unix seconds): what ends a session
    /// issued then, and an agent run started then on that person's behalf (T-6226).
    pub fn revoked_since(&self, subject: &str, at: i64) -> bool {
        self.revocations
            .read()
            .ok()
            .and_then(|marks| marks.get(subject).copied())
            .is_some_and(|mark| at <= mark)
    }

    /// The branding of this installation, loaded from `JC_BRANDING_FILE` or neutral defaults.
    pub fn branding(&self) -> Branding {
        Branding::load(self.config.branding_file.as_deref())
    }
}

/// A cookie session's claims kept on the server ([`AppState::keep_session_claims`]).
struct SessionClaims {
    /// Unix seconds the session ends; past it the entry goes.
    expires_at: i64,
    id_token: String,
    groups: Vec<String>,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Key {
        state.config.cookie_key.clone()
    }
}

/// The host the catalogue publisher reads each Endpoint's record and schemas on.
fn catalogue_host(config: &Config) -> String {
    config
        .api_base_url()
        .host_str()
        .unwrap_or("localhost")
        .to_owned()
}

/// One call held by `AppState::mcp_enter`; dropping it frees its slots.
pub struct InFlight {
    held: Arc<Mutex<HashMap<String, u32>>>,
    keys: Vec<String>,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let Ok(mut held) = self.held.lock() else {
            return;
        };
        for key in &self.keys {
            if let Some(count) = held.get_mut(key) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    held.remove(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::{now_unix, Identity};

    /// T-6549: the ID token stays on the server for one logout, per session, and an ended
    /// session's goes with the next login.
    #[test]
    fn an_id_token_is_kept_per_session_and_handed_out_once() {
        let state = AppState::new(Config::for_tests(), None);
        let now = now_unix();
        let session = |issued_at: i64, expires_at: i64, id_token: &str| Session {
            identity: Identity {
                client: None,
                subject: "f:1:demo.editor".into(),
                username: "demo.editor".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at,
            issued_at,
            signed_in_at: None,
            id_token: id_token.into(),
            access_expires_at: now + 300,
            refresh_token: None,
        };
        let ended = session(now - 7200, now - 1, "ended-session-token");
        let laptop = session(now - 60, now + 3600, "laptop-token");
        let phone = session(now - 30, now + 3600, "phone-token");
        state.keep_session_claims(&ended);
        state.keep_session_claims(&laptop);
        state.keep_session_claims(&phone);
        state.keep_session_claims(&session(now - 10, now + 3600, ""));
        // A refresh that brought no ID token keeps the session's, and its groups move.
        let mut refreshed = session(now - 30, now + 7200, "");
        refreshed.identity.groups = vec!["helsinki/editors".into()];
        state.keep_session_claims(&refreshed);
        assert_eq!(
            state.session_groups(&phone),
            Some(refreshed.identity.groups.clone())
        );
        assert_eq!(
            state.take_id_token(&ended),
            None,
            "pruned at the next login"
        );
        assert_eq!(
            state.take_id_token(&laptop).as_deref(),
            Some("laptop-token")
        );
        assert_eq!(state.take_id_token(&laptop), None, "handed out once");
        assert_eq!(state.take_id_token(&phone).as_deref(), Some("phone-token"));
        assert_eq!(
            state.take_id_token(&session(now - 10, now + 3600, "")),
            None
        );
    }

    /// F-R2POR1-011 (T-6225): a replica that does not hold the reconciler lock leads no
    /// background loop; one with no election leads.
    #[tokio::test]
    async fn a_follower_replica_leads_no_background_loop() {
        let alone = AppState::new(Config::for_tests(), None);
        assert!(alone.leads());
        let gitea = Arc::new(
            GiteaClient::new(
                "http://localhost:3000".parse().expect("a url"),
                "test-owner",
                "test-repo",
                "token",
            )
            .expect("a client"),
        );
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://follower@localhost/none")
            .expect("a lazy pool");
        let follower = Syncer::new(gitea, Arc::new(Mirror::new()))
            .with_leadership(Arc::new(Leadership::reconciler(pool)));
        let state = AppState::new(Config::for_tests(), None).with_syncer(Arc::new(follower));
        assert!(!state.leads());
    }

    #[test]
    fn mcp_slots_hold_until_the_guard_drops_and_leave_no_key_behind() {
        let state = AppState::new(Config::for_tests(), None);
        let slots = [("person", 2), ("connector", 1)];
        let first = state.mcp_enter(&slots).expect("free");
        assert_eq!(state.mcp_enter(&slots).err(), Some(1));
        let other = state
            .mcp_enter(&[("person", 2), ("b", 1)])
            .expect("person has one");
        assert_eq!(state.mcp_enter(&[("person", 2), ("c", 1)]).err(), Some(0));
        drop(first);
        drop(other);
        assert!(state.mcp_in_flight.lock().expect("lock").is_empty());
        assert!(state.mcp_enter(&slots).is_ok());
    }

    fn session(subject: &str, issued_at: i64) -> Session {
        Session {
            identity: Identity {
                client: None,
                subject: subject.into(),
                username: "demo.steward".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: issued_at + 600,
            issued_at,
            signed_in_at: None,
            id_token: String::new(),
            access_expires_at: issued_at + 600,
            refresh_token: None,
        }
    }

    fn forge() -> Syncer {
        let client = GiteaClient::new("http://127.0.0.1:9".parse().expect("url"), "o", "r", "t")
            .expect("client");
        Syncer::new(Arc::new(client), Arc::new(Mirror::default()))
    }

    /// T-4717: the publisher reads Endpoints on the data host; the Portal host answered each
    /// read with a 308 to it, two public edge requests per read.
    #[test]
    fn the_catalogue_reads_endpoints_on_the_data_host() {
        let mut config = Config::for_tests();
        config.public_base_url = "https://portal.dev.example.org".parse().expect("url");
        assert_eq!(catalogue_host(&config), "api.dev.example.org");
        config.public_base_url = "https://data.example.org".parse().expect("url");
        assert_eq!(catalogue_host(&config), "data.example.org");
    }

    /// T-4470: the parts `from_config` was cut into, each on a bare configuration.
    #[tokio::test]
    async fn without_a_database_or_a_cluster_nothing_is_attached() {
        let mut state = AppState::new(Config::for_tests(), None);
        state.attach_stores(&None);
        assert!(!state.agents.is_durable());
        state.attach_run_clients();
        assert!(state.kube.is_none() && state.app_tests.is_none());
        state.attach_wasm_apps();
        assert!(state.wasm_apps.is_none());
        // A database without its bucket is neither: the wasm Apps need both.
        let mut config = Config::for_tests();
        config.apps_db = Some(crate::config::AppsDbSettings {
            url: "postgres://apps@db/apps".into(),
            shards: 2,
            bucket: "apps".into(),
            host_namespace: None,
        });
        config.artifact_store = None;
        let mut state = AppState::new(config, None);
        state.attach_wasm_apps();
        assert!(state.wasm_apps.is_none());
    }

    #[tokio::test]
    async fn the_space_writers_keep_the_views_and_the_drift_watch_they_were_given() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _ = state.wire_space_writers(forge());
        assert!(state.views.is_none() && state.drift_watch.is_none());

        let mut config = Config::for_tests();
        config.broker_url = Some("http://broker:1026".into());
        config.broker_admin_token_file = Some("/nonexistent/token".into());
        let mut state = AppState::new(config, None);
        let _ = state.wire_space_writers(forge());
        assert!(
            state.views.is_some(),
            "a broker and its token file write the views"
        );
        assert!(state.drift_watch.is_none(), "no gateway, no drift watch");
    }

    #[tokio::test]
    async fn back_channel_logout_revokes_older_sessions_only() {
        let state = AppState::new(Config::for_tests(), None);
        let now = now_unix();
        let old = session("sub-1", now - 10);
        let fresh = session("sub-1", now + 10);
        assert!(!state.is_revoked(&old));

        state.revoke_subject("sub-1", now).await;
        assert!(
            state.is_revoked(&old),
            "session issued before the logout must be refused"
        );
        assert!(!state.is_revoked(&fresh), "a later login must still work");
        assert!(
            !state.is_revoked(&session("sub-2", now - 10)),
            "other users are untouched"
        );
    }

    /// JC2-19 review: a token refreshed after a logout mark keeps its SSO sign-in
    /// (`signed_in_at`) from before the mark, and still answers: revocation reads its `iat`.
    #[tokio::test]
    async fn a_refreshed_token_after_a_logout_mark_at_its_sign_in_still_answers() {
        let state = AppState::new(Config::for_tests(), None);
        let now = now_unix();
        state.revoke_subject("sub-1", now - 100).await;
        let mut refreshed = session("sub-1", now);
        refreshed.signed_in_at = Some(now - 100);
        assert!(!state.is_revoked(&refreshed));
        assert_eq!(refreshed.started_at(), now - 100);
    }

    /// T-0980: without a database the mark is this process's alone, and saying so is the point
    /// of the two paths — neither may panic or lose the in-memory mark.
    #[tokio::test]
    async fn a_portal_without_a_database_still_marks_and_reads_back_nothing() {
        let state = AppState::new(Config::for_tests(), None);
        let now = now_unix();
        state.revoke_subject("sub-1", now).await;
        state.load_revocations().await;
        assert!(state.is_revoked(&session("sub-1", now - 1)));
    }

    /// The mark only ever moves forward: a later logout refuses more, an earlier one refuses
    /// nothing it did not already.
    #[tokio::test]
    async fn a_mark_never_moves_backwards() {
        let state = AppState::new(Config::for_tests(), None);
        let now = now_unix();
        state.revoke_subject("sub-1", now).await;
        state.revoke_subject("sub-1", now - 100).await;
        assert!(
            state.is_revoked(&session("sub-1", now - 1)),
            "the later mark stands"
        );
    }
}
