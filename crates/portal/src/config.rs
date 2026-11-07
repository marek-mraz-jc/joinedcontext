use axum_extra::extract::cookie::Key;
use std::net::SocketAddr;
use std::time::Duration;
use url::Url;

pub use crate::auth::client_auth::ClientAuth;

/// Portal server configuration. Secrets are redacted in `Debug` so a config dump
/// never puts a client secret or a cookie key into the log (CC-40).
/// Where the project's Grafana is and who the Portal is to it (T-6509).
#[derive(Clone)]
pub struct GrafanaConfig {
    /// The server inside the cluster, with its path prefix (`JC_PORTAL_GRAFANA_URL`).
    pub url: String,
    /// The same server as a person's browser reaches it (`JC_PORTAL_GRAFANA_PUBLIC_URL`).
    pub public_url: String,
    /// Grafana's local administrator (`JC_PORTAL_GRAFANA_USER`).
    pub user: String,
    /// Its password (`JC_PORTAL_GRAFANA_PASSWORD`), which leaves the Portal only to Grafana.
    pub password: String,
}

impl std::fmt::Debug for GrafanaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrafanaConfig")
            .field("url", &self.url)
            .field("public_url", &self.public_url)
            .field("user", &"[redacted]")
            .field("password", &"[redacted]")
            .finish()
    }
}

#[derive(Clone)]
pub struct Config {
    /// The address to listen on (`JC_PORTAL_BIND`, default `0.0.0.0:8080`).
    pub bind: SocketAddr,
    /// Where the internal listener binds (`JC_INTERNAL_BIND`, default `0.0.0.0:9090`). APISIX
    /// routes nothing to it and a NetworkPolicy opens it to the named workloads alone (AG-52).
    /// It opens whenever the Portal verifies tokens: every route on it answers only a workload
    /// token of the realm, and the gateway's revocations poll needs it beside every gateway
    /// that verifies tokens, agent runner or not (T-5869).
    pub internal_bind: SocketAddr,
    /// The address a browser reaches this Portal at (`JC_PORTAL_PUBLIC_URL`, default
    /// `http://localhost:8080`). Every redirect URI, every link the Portal writes into a merge
    /// request and the host a generated application is served on are derived from it.
    pub public_base_url: Url,
    /// The realm humans sign in against: `JC_OIDC_ISSUER`, `JC_OIDC_CLIENT_ID` and one proof of
    /// the client, `JC_OIDC_CLIENT_SECRET` (a secret) or `JC_OIDC_CLIENT_ASSERTION_FILE` (the
    /// pod's projected ServiceAccount token, PF-47), all together or none, plus the optional
    /// `JC_OIDC_CA_FILE` for a realm behind a private CA.
    ///
    /// `None` disables login: no session can be minted, so every protected route
    /// answers 401. Configuration is fail-closed, never fail-open.
    pub oidc: Option<OidcConfig>,
    /// Whether an `X-Access-Token` header is believed to come from the APISIX edge and is
    /// verified as if it were `Authorization: Bearer` (ADR-N-019, AP-28). The deployment sets
    /// it behind the edge, which strips the header from every client request first; a Portal
    /// without an edge in front leaves it off and the header is ignored
    /// (`JC_TRUST_EDGE_TOKEN`, the literal string `true` to turn it on; default `false`).
    pub trust_edge_token: bool,
    /// Keycloak clients an MCP connector holds a person's token through (AG-63, AG-82, T-5610):
    /// such a bearer writes only through `/api/v1/mcp`, which asks the person first, and REST
    /// refuses its writes. Always [`CONNECTOR_CLIENTS`], plus any `JC_PORTAL_CONNECTOR_CLIENTS`
    /// names (comma-separated): the setting adds clients, it cannot open the door again.
    pub connector_clients: Vec<String>,
    /// Whether the Dashboards section is shown (`JC_PORTAL_DASHBOARDS`, the literal string
    /// `true` to show it; default hidden, the owner's call of 2026-09-25, T-2874). Hidden, the
    /// navigation leaves it out, its page sends a visit to the project and the assistant does
    /// not offer the path; the kind, its manifests and its API stay, so turning it on loses
    /// nothing.
    pub dashboards: bool,
    /// What the deployment says it switched on, for the organization setup page (T-2748).
    pub setup: SetupStatements,
    /// The key every session cookie is sealed with (`JC_PORTAL_COOKIE_KEY`, at least 64
    /// bytes). A secret. Required once a realm is configured (`JC_OIDC_ISSUER`): a Portal people
    /// sign in to refuses to start without it, since an ephemeral key ends every session at each
    /// restart and differs per replica. Without a realm nobody signs in, and an ephemeral key
    /// only signs the unsubscribe links.
    pub cookie_key: Key,
    /// Keys a session cookie may still be sealed with, during a rotation
    /// (`JC_PORTAL_COOKIE_KEY_PREVIOUS`, comma-separated, each held to the same length as the
    /// active one; T-0973). Secrets.
    ///
    /// A cookie key is the Portal's alone, so unlike the webhook secret it can change in one
    /// step — but every signed-in person's cookie is sealed with the old one, and swapping the
    /// key without a window signs everybody out. The active key seals; these only open, and a
    /// key stays here for as long as a session sealed with it may live.
    pub cookie_keys_previous: Vec<Key>,
    /// How often the reconciler re-reads the configuration repository
    /// (`JC_PORTAL_SYNC_INTERVAL`, whole seconds, default `60`).
    pub sync_interval: Duration,
    /// The secret the forge signs its webhook calls with (`JC_GITEA_WEBHOOK_SECRET`). A
    /// secret. `None` leaves the hook route refusing every call, so a change is picked up at
    /// the next sync rather than at the push.
    pub gitea_webhook_secret: Option<String>,
    /// The secret this Portal accepted before the current one, during a rotation
    /// (`JC_GITEA_WEBHOOK_SECRET_PREVIOUS`). A secret (T-0982).
    ///
    /// A webhook secret is shared with the forge, so the two sides cannot change at the same
    /// instant: the new one is written here as the active secret and the old one stays accepted
    /// until the forge's own hook is updated, then it is removed. Without it a rotation is a
    /// window in which every push is refused, which is why nobody rotates.
    pub gitea_webhook_secret_previous: Option<String>,
    /// Base URL of a project's Bento pipeline runner with `{project}` still in it, e.g.
    /// `http://pipeline-runner.{project}-pipeline-runner.svc.cluster.local:4195`
    /// (`JC_PORTAL_PIPELINE_RUNNER_URL`). `None` leaves the metrics route answering 503
    /// instead of guessing a service name.
    pub pipeline_runner_url: Option<String>,
    /// Whether every stream presents its own pipeline's identity through the runner's token
    /// sidecar (`JC_PORTAL_PIPELINE_IDENTITY=pipeline`) rather than its project's `pipelines`
    /// account (`project`, the default), PL-19. Set once the federated mechanism is seen working.
    pub pipeline_identity: bool,
    /// How many batch-lane and short-lane pipeline runs the runner holds at once
    /// (`JC_PORTAL_PIPELINES_BATCH_CONCURRENCY`, `JC_PORTAL_PIPELINES_SHORT_CONCURRENCY`, default 1
    /// each, PL-78).
    pub pipelines_concurrency: (usize, usize),
    /// The namespace the pipelines' own Kubernetes ServiceAccounts live in
    /// (`JC_PORTAL_PIPELINE_NAMESPACE`, `{instance}-pipeline-identities`): set, the reconciler
    /// keeps one there per Pipeline and gives each derived account its federated client (PL-19,
    /// T-1508). `pipeline_identity` needs it, since a stream would otherwise present an account
    /// nothing created.
    pub pipeline_namespace: Option<String>,
    /// The identities namespace the Apps' job principals live in
    /// (`JC_PORTAL_APP_IDENTITY_NAMESPACE`): set, the reconciler keeps one Kubernetes
    /// ServiceAccount there per `wasm` App with jobs and gives its principal `appjob-{app}` a
    /// federated client whose one audience is the App's Endpoint (AP-159, T-3539). Unset, no job
    /// gets a token.
    pub app_identity_namespace: Option<String>,
    /// The namespace the MQTT edge brokers run in (`JC_PORTAL_MQTT_NAMESPACE`,
    /// `{instance}-mqtt`): set, the reconciler applies one release per `MqttBroker` there from the
    /// deployment's template (ADR-N-048, T-3994); unset, it applies none and says so.
    pub mqtt_namespace: Option<String>,
    /// The Context Gateway's namespace (`JC_PORTAL_GATEWAY_NAMESPACE`, `{instance}-context-gateway`):
    /// set, the reconciler writes each space's resolved `spec.personalData.keyRef` there as the
    /// Secret `personal-data-keys`, which the gateway mounts at `JC_GATEWAY_PERSONAL_KEYS`
    /// (DM-121); unset, the keys reach only the Portal's own data subject lookup.
    pub gateway_namespace: Option<String>,
    /// The platform host the context spaces are served on, which is where a declared
    /// `Subscription` is written (`JC_PORTAL_GATEWAY_URL`,
    /// `/cs/{space}/ngsi-ld/v1/subscriptions`, T-0931). `None`
    /// leaves subscriptions read from the repository and written nowhere.
    pub gateway_url: Option<String>,
    /// A headless Service naming every gateway pod (`JC_PORTAL_GATEWAY_PODS_URL`, one address
    /// record per pod): a space move copies only once each pod's `/metrics` shows the hold
    /// (F-POR4-014). `None` asks `gateway_url` alone, right for one gateway replica.
    pub gateway_pods_url: Option<String>,
    /// The Keycloak client the Context Gateway holds (`JC_PORTAL_GATEWAY_CLIENT_ID`), which
    /// is the only caller `GET /internal/previews` answers (PF-46, AG-52). `None` leaves that route refusing every call:
    /// a Portal that was not told whose token to expect must not fall back to trusting the
    /// NetworkPolicy alone.
    pub gateway_client_id: Option<String>,
    /// The Keycloak client `jc-agent-proxy` holds (`JC_PORTAL_AGENT_PROXY_CLIENT_ID`), which is
    /// the only caller the run callbacks on the internal listener answer (AG-52, T-2271). It replaced `JC_AGENT_PROXY_TOKEN`, one string both
    /// sides held. `None` leaves those routes refusing every call.
    pub agent_proxy_client_id: Option<String>,
    /// The Keycloak client the pipeline runner holds (`JC_PORTAL_PIPELINE_RUNNER_CLIENT_ID`), which
    /// `POST /internal/pipeline-tests/{id}` answers beside the running test's own project's
    /// `pipelines` client (AG-52, T-2271, T-3193). It opens no pipeline's outcome or rejection
    /// route: those answer only their own project's pipeline clients. `None` admits the test's
    /// project alone.
    pub pipeline_runner_client_id: Option<String>,
    /// The Keycloak client an invitation's link returns through
    /// (`JC_PORTAL_INVITATION_CLIENT_ID`): one whose redirect URIs already cover the Portal's
    /// pages, so the invitee lands on the project's home page once their password is set
    /// (PF-108). `None` leaves the link ending on the realm's own page.
    pub invitation_client_id: Option<String>,
    /// The context broker as the Portal reaches it inside the cluster, which is where a
    /// declared `ContextSourceRegistration` is written, in the tenant of its hub space
    /// (`POST /ngsi-ld/v1/csourceRegistrations`, T-0345, SP-08). It is also the address the
    /// registration tells the broker to read a member at, because a hub reads a member's tenant
    /// on the same broker (PF-48; `JC_PORTAL_BROKER_URL`). `None` leaves registrations read
    /// from the repository and written nowhere.
    pub broker_url: Option<String>,
    /// The address of every broker but `main` (`JC_PORTAL_BROKER_URL_TEMPLATE`, PF-111), with
    /// `{broker}` in the authority replaced by the `ContextBroker`'s name. `None` leaves a space
    /// on another broker unwritten and uncounted, never sent to `main`.
    pub broker_url_template: Option<String>,
    /// Where the broker's admin token is mounted from its Secret
    /// (`JC_PORTAL_BROKER_ADMIN_TOKEN_FILE`), read on every run and sent only as the bearer of
    /// the `/q` calls that write every Endpoint's and Context Space's view and every space's
    /// policy rule document (T-3640), and of the `ContextSourceRegistration` writes and deletes,
    /// which carry no subject (T-5032). With the broker's address set it turns the view writer on;
    /// `None` leaves the Endpoints on the gateway and writes no view.
    pub broker_admin_token_file: Option<std::path::PathBuf>,
    /// The organization's domain, the third segment of every URN this instance writes
    /// (`JC_PORTAL_ORG_DOMAIN`; `urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}`).
    pub org_domain: Option<String>,
    /// Where a pipeline test's harness posts what it produced (PL-43): the Portal's internal
    /// listener as the project's runner reaches it, e.g. `http://portal-internal:9090`
    /// (`JC_PORTAL_PIPELINE_TEST_CAPTURE_URL`). `None` means the test route answers 503.
    pub pipeline_test_capture_url: Option<String>,
    /// Base URL of the stateless Model Tools service (`JC_PORTAL_MODEL_TOOLS_URL`), e.g.
    /// `http://model-tools.tools.svc.cluster.local:8080`. `None` leaves the LinkML preview
    /// routes answering 503 instead of guessing a service name (DM-18).
    pub model_tools_url: Option<String>,
    /// The Smart Data Models mirror in the image (`JC_PORTAL_SDM_MIRROR_DIR`, T-6353). `None`, or
    /// a directory that does not read, leaves every import to Model Tools.
    pub sdm_mirror_dir: Option<String>,
    /// Base URL of the `portal-ui` bundle the pages are read from (`JC_PORTAL_UI_URL`), e.g.
    /// `http://portal-ui.jc-portal.svc.cluster.local:8080`, without a trailing slash (T-2485).
    /// `None` serves the bundle built into the binary, or the placeholder page without one.
    pub ui_url: Option<String>,
    /// Base URL of the `jc-functions` runtime (`JC_FUNCTIONS_URL`), e.g.
    /// `http://jc-functions.jc-system.svc.cluster.local:8080`. `None` leaves the function routes
    /// answering 503 (SDK-23).
    pub functions_url: Option<String>,
    /// Base URL of the knowledge assistant `jc-assistant` (`JC_PORTAL_KNOWLEDGE_URL`), e.g.
    /// `http://jc-assistant.jc-system.svc.cluster.local:8080`, whose administration paths the
    /// knowledge routes ask with the Portal's own token (API/01 §34, T-3057). `None` leaves those
    /// routes answering 503.
    pub knowledge_url: Option<String>,
    /// The project's Grafana (`JC_PORTAL_GRAFANA_URL` and friends, T-6509); `None` where the
    /// deployment runs none, and the Grafana page says so.
    pub grafana: Option<GrafanaConfig>,
    /// Root of the built app bundles, one directory per app (`JC_PORTAL_APPS_DIR`). `None`
    /// leaves every
    /// `/apps/{name}/` path answering 404 rather than reading a guessed directory (AP-14).
    pub apps_dir: Option<String>,
    /// The mail relay App mail is sent through (`JC_PORTAL_SMTP_URL`), a secret the deployment
    /// takes from a `secretRef`: `smtp://host:1025` without TLS for the in-cluster catcher,
    /// `smtps://user:password@host:465` or `smtp://user:password@host:587?tls=required` for a
    /// relay (T-3583, AP-168). `None` leaves `POST /api/services/email/send` answering 503.
    pub smtp_url: Option<String>,
    /// The key a data subject is hashed with for the erasure requests and their log
    /// (`JC_PORTAL_ERASURE_KEY`, a `secretRef`, DM-124): its own secret, so rotating the artifact
    /// store's credential leaves every logged subject hash matching. `None` falls back to
    /// `JC_PORTAL_ARTIFACT_STORE_SECRET_KEY`, the key earlier hashes were made with.
    pub erasure_key: Option<String>,
    /// The address App mail is sent from (`JC_PORTAL_MAIL_FROM`, e.g.
    /// `noreply@example.org`); the App's title is the display name beside it. Required
    /// with `JC_PORTAL_SMTP_URL`.
    pub mail_from: Option<lettre::Address>,
    /// Where this replica keeps the builds it fetched from the package registry, one
    /// `{name}/{hex}` directory per build (`JC_PORTAL_APPS_CACHE_DIR`, AP-102); it must be
    /// writable, and `{apps_dir}` need not be. `None` fetches nothing, and an App whose
    /// `status.build` names a build keeps serving the bundle the image ships.
    pub apps_cache_dir: Option<String>,
    /// The apex every App's own origin sits under (`JC_PORTAL_APPS_URL`, e.g. `https://{domain}`;
    /// AP-26, AP-133, ADR-N-037): App `name` is served only on `{name}.apps.{domain}` and
    /// answered with a `308` there on any other host, above all the Portal's own, where it would
    /// call the Portal API with the viewer's session (T-2476), and another App's. `None` serves
    /// on every host, as a Portal without an edge in front of it does.
    pub apps_url: Option<Url>,
    /// The file the deployment renders `global.branding` into (`JC_BRANDING_FILE`; UI-30,
    /// OPS-46). `None` serves
    /// neutral joinedcontext defaults, which is what an installation without branding looks
    /// like; it is never an error.
    pub branding_file: Option<String>,
    /// The deployment's own open-data catalogue, called in the cluster (T-5314):
    /// `JC_PORTAL_CKAN_URL` is the public URL a CkanInstance's `spec.url` names (e.g.
    /// `https://opendata.example.org`), `JC_PORTAL_CKAN_API_URL` the catalogue's Service the
    /// reconciler calls its API at instead (e.g. `http://ckan.ckan.svc.cluster.local:5000`), so
    /// publishing does not spend the public edge's limit. Both or neither; `None` calls every
    /// catalogue at its public URL.
    pub ckan_api_route: Option<(Url, Url)>,
    /// The directory the ConfigMap `jc-validation-results` is mounted at (`JC_HEALTH_DIR`;
    /// OPS-53): one digest per validation check, read on every request to
    /// `/api/v1/organization/health`. `None` answers an empty list; it is never an error.
    pub health_dir: Option<String>,
    /// PostgreSQL connection string of the preferences tier (`JC_PORTAL_DATABASE_URL`,
    /// UI-09). A secret: it carries a password, so it is redacted in `Debug`. `None` runs the Portal without preferences: those routes answer
    /// 503, everything else works.
    pub database_url: Option<String>,
    /// The group (or realm role) whose members may do everything everywhere, so the first
    /// `RoleBinding` can be written into an empty repository (`JC_PORTAL_BOOTSTRAP_ADMINS`,
    /// default `platform-admins`; T-0526, PF-50).
    pub bootstrap_admins: String,
    /// Every decision door asks for a second factor proven in the session (T-5567, AUT-2).
    /// On unless `JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR` is the literal `false` (T-6083: a dev
    /// deployment whose people sign in with a password alone); any other value keeps it on, and
    /// the Portal logs a warning at boot when it is off. `Config::for_tests` leaves it off so a
    /// test session that decides does not need a step-up of its own.
    pub decisions_need_second_factor: bool,
    /// The usernames the Portal's own live journeys sign in as, the only people whose runs may
    /// carry `X-JC-Run-Origin: journey` (`JC_PORTAL_JOURNEY_USERS`, comma-separated; default
    /// none, so no one can mark a run; AG-93, T-2816).
    pub journey_users: Vec<String>,
    /// The client the reconciler manages the realm's groups with: a `ServiceAccount` client
    /// holding `manage-users` and `query-groups` of `realm-management` and nothing else
    /// (`JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID` and one of `JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET`, a
    /// secret, `JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ASSERTION_FILE`, a projected ServiceAccount
    /// token, or `JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT`, a ServiceAccount of this
    /// namespace whose token the Portal mints per request; together or neither; PF-63, PF-47). `None` leaves the `Group` manifests read and the realm written by nobody.
    pub keycloak_admin: Option<(String, ClientAuth)>,
    /// The operator's bounds on what an Organization may set (`JC_PORTAL_ORGANIZATION_BOUNDS_FILE`,
    /// the file the deployment renders `portal.organizationBounds` into; PF-97, ADR-N-035). No
    /// file keeps every entry at the catalog's built-in bound.
    pub organization_bounds: jc_core::kinds::OrganizationBounds,
    /// The operator's price table for Administration → Costs (`JC_PORTAL_COST_PRICES_FILE`, the
    /// file the deployment renders `portal.costPrices` into; T-5126): the currency, the rate
    /// for one US dollar, each node, volume and fixed item with its monthly price. No file leaves
    /// the page saying no table is set.
    pub cost_prices: Option<crate::costs::CostPrices>,
    /// Where an App's four Kubernetes objects are applied (`JC_PORTAL_APPS_NAMESPACE` with
    /// `JC_PORTAL_ORG_DOMAIN`; AP-13, AP-18, T-0411). `None` leaves
    /// the reconciler reading apps and applying nothing, which is what a Portal outside a
    /// cluster does; it is never a guess, because guessing a namespace here would mean writing
    /// a Deployment into somebody else's.
    pub app_settings: Option<crate::apps::reconciler::Settings>,
    /// Where an App's build pods run and on which runner images (`JC_PORTAL_BUILD_NAMESPACE`
    /// with `JC_PORTAL_BUILD_IMAGE_NODE`, `JC_PORTAL_BUILD_IMAGE_RUST` and the forge's API base;
    /// `JC_PORTAL_BUILD_CACHE_SIZE`, default `1Gi`; AP-130, AP-131). `None` starts no build pod,
    /// and an App's build waits in the forge's queue.
    pub build_pods: Option<crate::apps::build_pods::Settings>,
    /// Where a builder run's workspace is scheduled and how the proxy reaches this Portal
    /// (AG-33, AG-40). `None` leaves every agent-run route answering 503: without a namespace
    /// to schedule into and a proxy for the workspace to speak to, a run has nowhere to happen.
    pub agent_settings: Option<AgentSettings>,
    /// Where a run tests each version before it offers publication (SDK-38): the sandbox
    /// namespace (`JC_PORTAL_APP_TESTS_NAMESPACE`) and the lane's builder image pinned by digest
    /// (`JC_PORTAL_APP_TESTS_IMAGE`), both or neither, and the rust runner for a `wasm` App's
    /// server tests (`JC_PORTAL_APP_TESTS_RUST_IMAGE`, optional). `None` runs no tests in the run, says so on
    /// it, and leaves the build lane's check as the only one (SDK-24).
    pub app_tests: Option<crate::agents::sandbox::SandboxSettings>,
    /// Where basemap tiles and styles come from (AP-67). `None` disables the basemap route:
    /// tile requests answer 404 and generated maps render on a plain canvas.
    pub basemap: Option<BasemapConfig>,
    /// The object store's public origin (`JC_PORTAL_APPS_STORE_ORIGIN`, `https://files.{domain}`),
    /// where the browser sends a `wasm` App's presigned uploads and downloads (AP-145, T-3359):
    /// the one origin `connect-src` gains, for the Apps that declare blob storage. `None` adds
    /// nothing, and such an App's uploads are refused by its policy.
    pub apps_store_origin: Option<String>,
    /// The artifact store and the root credential the reconciler mints per-organization
    /// credentials with (PF-32, ADR-N-015). `None` leaves the store untouched: a Portal outside
    /// a cluster reconciles a repository and issues nothing.
    pub artifact_store: Option<crate::artifact_store::Settings>,
    /// The apps database and bucket of the `wasm` Apps (AP-149, AP-151); `None` without
    /// `JC_PORTAL_APPS_DB_URL`.
    pub apps_db: Option<AppsDbSettings>,
    /// Which backend resolves a pipeline's `secretRef`s (PL-15, CC-06). A deployment setting,
    /// never a manifest field; `None` leaves a pipeline that declares one undeployed.
    pub pipeline_secrets: Option<crate::pipeline_secrets::Backend>,
}

/// The installation's own part of an organization's setup, as the deployment renders it from the
/// values that switch each on (T-2748, API/01 §25). Unset is "not said", which the setup page
/// shows as not done: it never claims what nobody stated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupStatements {
    /// The realm's login theme (`JC_SETUP_LOGIN_THEME`); unset or blank says nothing about it.
    pub login_theme: Option<String>,
    /// The realm sends mail (`JC_SETUP_SMTP`, the literal `true`).
    pub smtp: bool,
    /// The databases are backed up to an object store (`JC_SETUP_BACKUPS`, the literal `true`).
    pub backups: bool,
}

impl SetupStatements {
    fn from_vars(lookup: &impl Fn(&str) -> Option<String>) -> Self {
        let yes = |key: &str| lookup(key).is_some_and(|v| v.trim() == "true");
        Self {
            login_theme: lookup("JC_SETUP_LOGIN_THEME")
                .map(|theme| theme.trim().to_owned())
                .filter(|theme| !theme.is_empty()),
            smtp: yes("JC_SETUP_SMTP"),
            backups: yes("JC_SETUP_BACKUPS"),
        }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_base_url", &self.public_base_url.as_str())
            .field("oidc", &self.oidc)
            .field("trust_edge_token", &self.trust_edge_token)
            .field("connector_clients", &self.connector_clients)
            .field(
                "decisions_need_second_factor",
                &self.decisions_need_second_factor,
            )
            .field("dashboards", &self.dashboards)
            .field("setup", &self.setup)
            .field("cookie_key", &"[redacted]")
            .field(
                "cookie_keys_previous",
                &format_args!("[{} redacted]", self.cookie_keys_previous.len()),
            )
            .field("sync_interval", &self.sync_interval)
            .field(
                "gitea_webhook_secret",
                &self.gitea_webhook_secret.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "gitea_webhook_secret_previous",
                &self
                    .gitea_webhook_secret_previous
                    .as_ref()
                    .map(|_| "[redacted]"),
            )
            .field("pipeline_runner_url", &self.pipeline_runner_url)
            .field("smtp_url", &self.smtp_url.as_ref().map(|_| "[redacted]"))
            .field(
                "erasure_key",
                &self.erasure_key.as_ref().map(|_| "[redacted]"),
            )
            .field("mail_from", &self.mail_from)
            .field("pipeline_identity", &self.pipeline_identity)
            .field("pipelines_concurrency", &self.pipelines_concurrency)
            .field("pipeline_namespace", &self.pipeline_namespace)
            .field("app_identity_namespace", &self.app_identity_namespace)
            .field("mqtt_namespace", &self.mqtt_namespace)
            .field("gateway_namespace", &self.gateway_namespace)
            .field("gateway_url", &self.gateway_url)
            .field("gateway_pods_url", &self.gateway_pods_url)
            .field("gateway_client_id", &self.gateway_client_id)
            .field("agent_proxy_client_id", &self.agent_proxy_client_id)
            .field("pipeline_runner_client_id", &self.pipeline_runner_client_id)
            .field("invitation_client_id", &self.invitation_client_id)
            .field("broker_url", &self.broker_url)
            .field("broker_url_template", &self.broker_url_template)
            .field("broker_admin_token_file", &self.broker_admin_token_file)
            .field("org_domain", &self.org_domain)
            .field("pipeline_test_capture_url", &self.pipeline_test_capture_url)
            .field("model_tools_url", &self.model_tools_url)
            .field("sdm_mirror_dir", &self.sdm_mirror_dir)
            .field("ui_url", &self.ui_url)
            .field("functions_url", &self.functions_url)
            .field("knowledge_url", &self.knowledge_url)
            .field("grafana", &self.grafana)
            .field("apps_dir", &self.apps_dir)
            .field("apps_cache_dir", &self.apps_cache_dir)
            .field("apps_url", &self.apps_url.as_ref().map(Url::as_str))
            .field("artifact_store", &self.artifact_store)
            .field("apps_db", &self.apps_db)
            .field("pipeline_secrets", &self.pipeline_secrets)
            .field("branding_file", &self.branding_file)
            .field("ckan_api_route", &self.ckan_api_route)
            .field("health_dir", &self.health_dir)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[redacted]"),
            )
            .field("app_settings", &self.app_settings)
            .field("build_pods", &self.build_pods)
            .field("agent_settings", &self.agent_settings)
            .field("basemap", &self.basemap)
            .finish()
    }
}

/// Where an App's Kubernetes objects are applied, or `None` when this Portal applies none
/// (AP-13, AP-18, T-0411).
///
/// All three values are needed together: the namespace to write into, the organization domain
/// that becomes a policy's assigner, and the host the app's endpoint is served on. The host is
/// read off configuration the Portal already has, so an installation states two variables
/// rather than three, and any missing one leaves the converger off instead of guessing a
/// namespace and deploying into somebody else's.
/// Where the artifact store is and the root credential to mint with (PF-32, ADR-N-015).
///
/// `JC_PORTAL_ARTIFACT_STORE_ENDPOINT`, `JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY` and
/// `JC_PORTAL_ARTIFACT_STORE_SECRET_KEY` (the last two secrets) name the store and the root
/// credential; `JC_PORTAL_ARTIFACT_STORE_BUCKET` (default `jc-artifacts`) and
/// `JC_PORTAL_ARTIFACT_STORE_REGION` (default `us-east-1`) are the same in every installation
/// this platform deploys, so they have defaults.
///
/// Where the `wasm` Apps' database is and how the host is sharded (AP-149). The URL comes from
/// the deployment's secretRef `db-apps-admin`, never from a manifest; `Debug` never prints it.
#[derive(Clone)]
pub struct AppsDbSettings {
    pub url: String,
    /// How many shards the WASM host runs, `JC_PORTAL_WASM_SHARDS`, default 2.
    pub shards: u32,
    /// The bucket of `components/` and `apps/`, `JC_PORTAL_APPS_BUCKET`, default `apps`.
    pub bucket: String,
    /// The namespace the WASM host's shards run in, `JC_PORTAL_WASM_HOST_NAMESPACE`, where the
    /// reconciler writes each shard's placement and store key (AP-157, AP-158). Without it the
    /// reconciler writes neither and says so.
    pub host_namespace: Option<String>,
}

impl std::fmt::Debug for AppsDbSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppsDbSettings")
            .field("shards", &self.shards)
            .field("bucket", &self.bucket)
            .field("host_namespace", &self.host_namespace)
            .finish_non_exhaustive()
    }
}

/// A shard count that is no positive number is refused: the host would otherwise run a
/// different number of shards than the operator wrote, and placements would not match it.
fn apps_db_settings(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<AppsDbSettings>, ConfigError> {
    let present = |var: &str| lookup(var).filter(|v| !v.trim().is_empty());
    let Some(url) = present("JC_PORTAL_APPS_DB_URL") else {
        return Ok(None);
    };
    let shards = match present("JC_PORTAL_WASM_SHARDS") {
        None => 2,
        Some(v) => v
            .trim()
            .parse()
            .ok()
            .filter(|n: &u32| *n > 0)
            .ok_or_else(|| ConfigError::Invalid {
                var: "JC_PORTAL_WASM_SHARDS",
                reason: format!("'{}' is no positive whole number; write e.g. 2", v.trim()),
            })?,
    };
    Ok(Some(AppsDbSettings {
        url,
        shards,
        bucket: present("JC_PORTAL_APPS_BUCKET").unwrap_or_else(|| "apps".to_owned()),
        host_namespace: present("JC_PORTAL_WASM_HOST_NAMESPACE").map(|v| v.trim().to_owned()),
    }))
}

/// All-or-nothing on purpose: an endpoint without the root credential would sign every admin
/// request with nothing and log a refusal each sync, and a credential without an endpoint has
/// no store to reach. Missing means the reconciler issues no credentials at all, which is what
/// a Portal outside a cluster does.
fn artifact_store_settings(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Option<crate::artifact_store::Settings> {
    let present = |var: &str| lookup(var).filter(|v| !v.trim().is_empty());
    let endpoint = present("JC_PORTAL_ARTIFACT_STORE_ENDPOINT")?;
    let root_access_key = present("JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY")?;
    let root_secret_key = present("JC_PORTAL_ARTIFACT_STORE_SECRET_KEY")?;
    Some(crate::artifact_store::Settings {
        endpoint: endpoint.trim_end_matches('/').to_owned(),
        // The bucket and the region are the same in every installation this platform deploys
        // (Architecture/17 §4), so they have defaults; the endpoint and the credential never do.
        bucket: present("JC_PORTAL_ARTIFACT_STORE_BUCKET")
            .unwrap_or_else(|| "jc-artifacts".to_owned()),
        region: present("JC_PORTAL_ARTIFACT_STORE_REGION")
            .unwrap_or_else(|| "us-east-1".to_owned()),
        root_access_key,
        root_secret_key,
    })
}

/// Which secret backend the reconciler resolves a pipeline's references with (PL-15, CC-06).
///
/// `JC_PORTAL_SOPS_AGE_KEY_FILE` names the age key file and chooses SOPS; otherwise
/// `JC_PORTAL_OPENBAO_ADDR` and `JC_PORTAL_OPENBAO_ROLE` choose OpenBao, with
/// `JC_PORTAL_OPENBAO_JWT_PATH` (default
/// `/var/run/secrets/kubernetes.io/serviceaccount/token`) for the ServiceAccount token it logs
/// in with. The key file is a path to a secret, never the secret itself.
///
/// SOPS first, because the repository is a store this Portal already has in its hands every
/// sync and OpenBao is a component a deployment has to run. Naming neither is not an error: a
/// Portal without a backend refuses only the pipelines that declare a reference, and says so on
/// each of them.
fn pipeline_secret_backend(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Option<crate::pipeline_secrets::Backend> {
    let present = |var: &str| lookup(var).filter(|v| !v.trim().is_empty());
    if let Some(age_key_file) = present("JC_PORTAL_SOPS_AGE_KEY_FILE") {
        return Some(crate::pipeline_secrets::Backend::Sops {
            age_key_file: age_key_file.into(),
        });
    }
    let address = present("JC_PORTAL_OPENBAO_ADDR")?;
    Some(crate::pipeline_secrets::Backend::OpenBao {
        address,
        // The Kubernetes auth role this Portal logs in as. Named here rather than defaulted:
        // a role guessed wrong is a login refused on every sync with no line saying why.
        role: present("JC_PORTAL_OPENBAO_ROLE")?,
        jwt_path: present("JC_PORTAL_OPENBAO_JWT_PATH")
            .unwrap_or_else(|| "/var/run/secrets/kubernetes.io/serviceaccount/token".to_owned())
            .into(),
    })
}

/// `JC_PORTAL_APPS_URL`: an absolute `http(s)` origin with nothing after it. A path would be
/// dropped by the redirect anyway, so it is refused here rather than silently ignored.
/// An optional address that must be http(s): a typo is refused at startup, not on first use.
/// The Grafana the Portal saves visualizations into: all four variables or none, because half
/// of them is a page that promises a save it cannot make.
fn grafana_config(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<GrafanaConfig>, ConfigError> {
    let trimmed = |url: Option<String>| url.map(|url| url.trim_end_matches('/').to_owned());
    let url = trimmed(http_url(lookup, "JC_PORTAL_GRAFANA_URL")?);
    let public_url = trimmed(http_url(lookup, "JC_PORTAL_GRAFANA_PUBLIC_URL")?);
    let user = lookup("JC_PORTAL_GRAFANA_USER").filter(|v| !v.trim().is_empty());
    let password = lookup("JC_PORTAL_GRAFANA_PASSWORD").filter(|v| !v.is_empty());
    match (url, public_url, user, password) {
        (None, None, None, None) => Ok(None),
        (Some(url), Some(public_url), Some(user), Some(password)) => Ok(Some(GrafanaConfig {
            url,
            public_url,
            user,
            password,
        })),
        _ => Err(ConfigError::Invalid {
            var: "JC_PORTAL_GRAFANA_URL",
            reason: "set it with JC_PORTAL_GRAFANA_PUBLIC_URL, JC_PORTAL_GRAFANA_USER and \
                     JC_PORTAL_GRAFANA_PASSWORD, or none of the four"
                .into(),
        }),
    }
}

fn http_url(
    lookup: &impl Fn(&str) -> Option<String>,
    var: &'static str,
) -> Result<Option<String>, ConfigError> {
    lookup(var)
        .map(|raw| http_scheme(var, &raw).map(|_| raw))
        .transpose()
}

fn http_scheme(var: &'static str, raw: &str) -> Result<Url, ConfigError> {
    let url: Url = raw
        .parse()
        .map_err(|e: url::ParseError| ConfigError::Invalid {
            var,
            reason: e.to_string(),
        })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ConfigError::Invalid {
            var,
            reason: format!("scheme '{}' is not http or https", url.scheme()),
        });
    }
    Ok(url)
}

/// An optional namespace, trimmed; blank is unset, anything else must be a DNS-1123 label.
fn dns_label(
    lookup: &impl Fn(&str) -> Option<String>,
    var: &'static str,
) -> Result<Option<String>, ConfigError> {
    let Some(namespace) = lookup(var)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    if jc_core::names::validate_dns1123_label(&namespace).is_err() {
        return Err(ConfigError::Invalid {
            var,
            reason: format!("`{namespace}` is not a DNS-1123 label"),
        });
    }
    Ok(Some(namespace))
}

/// The login: issuer, client id and a proof together, or none of them.
fn oidc_config(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<OidcConfig>, ConfigError> {
    Ok(
        match (
            lookup("JC_OIDC_ISSUER"),
            lookup("JC_OIDC_CLIENT_ID"),
            ClientAuth::from_vars(
                &lookup,
                "JC_OIDC_CLIENT_SECRET",
                "JC_OIDC_CLIENT_ASSERTION_FILE",
            )?,
        ) {
            (None, None, None) => None,
            (Some(issuer), Some(client_id), Some(auth)) => Some(OidcConfig {
                issuer: issuer
                    .parse()
                    .map_err(|e: url::ParseError| ConfigError::Invalid {
                        var: "JC_OIDC_ISSUER",
                        reason: e.to_string(),
                    })?,
                client_id,
                auth,
                // Unreadable means misconfigured, not "carry on with fewer roots": a start-up
                // error names the file, a silent fallback would be a confusing 500 at login.
                extra_ca_pem: match lookup("JC_OIDC_CA_FILE") {
                    None => None,
                    Some(path) => Some(std::fs::read(&path).map_err(|e| ConfigError::Invalid {
                        var: "JC_OIDC_CA_FILE",
                        reason: format!("{path}: {e}"),
                    })?),
                },
                admin_url: match lookup("JC_KEYCLOAK_ADMIN_URL").filter(|v| !v.trim().is_empty()) {
                    None => None,
                    Some(raw) => {
                        let url: Url = raw.trim().parse().map_err(|e: url::ParseError| {
                            ConfigError::Invalid {
                                var: "JC_KEYCLOAK_ADMIN_URL",
                                reason: e.to_string(),
                            }
                        })?;
                        // An origin only: a path, query or user info would be pasted into every
                        // admin call, and a credential in it would reach the logs.
                        if !matches!(url.scheme(), "http" | "https")
                            || !url.username().is_empty()
                            || url.password().is_some()
                            || url.path() != "/"
                            || url.query().is_some()
                        {
                            return Err(ConfigError::Invalid {
                                var: "JC_KEYCLOAK_ADMIN_URL",
                                reason:
                                    "an http(s) origin, such as http://keycloak.keycloak.svc:8080"
                                        .to_string(),
                            });
                        }
                        Some(url)
                    }
                },
            }),
            _ => {
                return Err(ConfigError::Invalid {
                    var: "JC_OIDC_ISSUER",
                    reason: "JC_OIDC_ISSUER, JC_OIDC_CLIENT_ID and JC_OIDC_CLIENT_SECRET or \
                         JC_OIDC_CLIENT_ASSERTION_FILE must be set together"
                        .to_string(),
                })
            }
        },
    )
}

/// The session cookie's key and the keys a rotation still lets in.
fn cookie_keys(
    lookup: &impl Fn(&str) -> Option<String>,
    sign_in: bool,
) -> Result<(Key, Vec<Key>), ConfigError> {
    let cookie_key = match lookup("JC_PORTAL_COOKIE_KEY") {
        Some(material) if material.len() >= Config::MIN_COOKIE_KEY_LEN => {
            Key::from(material.as_bytes())
        }
        Some(_) => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_COOKIE_KEY",
                reason: format!("at least {} bytes required", Config::MIN_COOKIE_KEY_LEN),
            })
        }
        None if sign_in => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_COOKIE_KEY",
                reason: format!(
                    "required while JC_OIDC_ISSUER is set: give at least {} random bytes, \
                     e.g. from `openssl rand -base64 48`",
                    Config::MIN_COOKIE_KEY_LEN
                ),
            })
        }
        None => {
            tracing::warn!(
                "JC_PORTAL_COOKIE_KEY is unset: using an ephemeral key, nobody signs in to a \
                 Portal without a realm"
            );
            Key::generate()
        }
    };

    // The keys a rotation is still letting in. Several, comma-separated, because a second
    // rotation can start before the first one's sessions have all expired; each is held to
    // the same length as the active one, since a short key here would be the way in.
    let cookie_keys_previous = match lookup("JC_PORTAL_COOKIE_KEY_PREVIOUS") {
        Some(material) => {
            let mut keys = Vec::new();
            for part in material.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                if part.len() < Config::MIN_COOKIE_KEY_LEN {
                    return Err(ConfigError::Invalid {
                        var: "JC_PORTAL_COOKIE_KEY_PREVIOUS",
                        reason: format!("at least {} bytes required", Config::MIN_COOKIE_KEY_LEN),
                    });
                }
                keys.push(Key::from(part.as_bytes()));
            }
            keys
        }
        None => Vec::new(),
    };
    Ok((cookie_key, cookie_keys_previous))
}

/// `JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR` (T-6083): only the literal `false` turns the
/// decision doors' second-factor gate off, so a typo keeps AUT-2 in force.
fn decisions_need_second_factor(lookup: &impl Fn(&str) -> Option<String>) -> bool {
    let on = lookup("JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR").as_deref() != Some("false");
    if !on {
        tracing::warn!(
            "JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR is false: approve, reject, revoke and people \
             administration accept a password-only session (AUT-2 off, dev only)"
        );
    }
    on
}

/// The group reconciler's Keycloak client: an id and exactly one proof, or neither.
fn keycloak_admin(
    lookup: &impl Fn(&str) -> Option<String>,
    oidc: Option<&OidcConfig>,
) -> Result<Option<(String, ClientAuth)>, ConfigError> {
    // The reconciler's client is a second federated client in this pod: its proof is a
    // token of its own ServiceAccount, minted per request (PF-47, T-2868).
    let admin_account =
        lookup("JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT").filter(|v| !v.trim().is_empty());
    let admin_auth = match (
        ClientAuth::from_vars(
            &lookup,
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET",
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ASSERTION_FILE",
        )?,
        admin_account,
    ) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT",
                reason: "the group reconciler's client authenticates one way: a secret, a \
                         token file or a ServiceAccount"
                    .to_owned(),
            })
        }
        (Some(auth), None) => Some(auth),
        (None, Some(account)) => {
            let audience = oidc.map(|oidc| oidc.issuer.as_str().trim_end_matches('/'));
            let audience = audience.ok_or_else(|| ConfigError::Invalid {
                var: "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT",
                reason: "a minted token's audience is the realm, and JC_OIDC_ISSUER is unset"
                    .to_owned(),
            })?;
            Some(
                ClientAuth::minted(
                    std::path::Path::new(crate::apps::kube::SERVICE_ACCOUNT_DIR),
                    crate::apps::kube::IN_CLUSTER_API,
                    account.trim(),
                    audience,
                )
                .map_err(|reason| ConfigError::Invalid {
                    var: "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT",
                    reason,
                })?,
            )
        }
        (None, None) => None,
    };
    // Both halves or neither: an id without a proof would send an unauthenticated token
    // request every tick and log a refusal every time.
    let keycloak_admin = match (
        lookup("JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID").filter(|v| !v.trim().is_empty()),
        admin_auth,
    ) {
        (Some(id), Some(auth)) => Some((id, auth)),
        (Some(_), None) => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET",
                reason: "the group reconciler has a client id and no secret or token file"
                    .to_owned(),
            })
        }
        _ => None,
    };
    Ok(keycloak_admin)
}

fn apps_url(lookup: &impl Fn(&str) -> Option<String>) -> Result<Option<Url>, ConfigError> {
    let Some(raw) = lookup("JC_PORTAL_APPS_URL").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let invalid = |reason: &str| ConfigError::Invalid {
        var: "JC_PORTAL_APPS_URL",
        reason: reason.to_owned(),
    };
    let url: Url = raw
        .trim()
        .parse()
        .map_err(|e: url::ParseError| invalid(&e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid("an http(s) origin such as https://example.org"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("an origin only, without a path, query or fragment"));
    }
    Ok(Some(url))
}

/// The catalogue's public URL and the in-cluster address its API is called at (T-5314). The
/// public one is `https://` like every CkanInstance URL (EP-67); the API one may be plain
/// `http://` only on a cluster Service name, where the mesh carries the token, never across a
/// network the operator does not hold.
fn ckan_api_route(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<(Url, Url)>, ConfigError> {
    let set = |var: &str| {
        lookup(var)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (public, api) = match (set("JC_PORTAL_CKAN_URL"), set("JC_PORTAL_CKAN_API_URL")) {
        (None, None) => return Ok(None),
        (Some(public), Some(api)) => (public, api),
        (None, Some(_)) => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_CKAN_URL",
                reason: "JC_PORTAL_CKAN_API_URL is set, so the public catalogue URL it stands \
                         in for is needed too, e.g. https://opendata.example.org"
                    .into(),
            })
        }
        (Some(_), None) => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_CKAN_API_URL",
                reason: "JC_PORTAL_CKAN_URL is set, so the in-cluster address of its API is \
                         needed too, e.g. http://ckan.ckan.svc.cluster.local:5000"
                    .into(),
            })
        }
    };
    let parse = |var: &'static str, raw: &str| -> Result<Url, ConfigError> {
        let invalid = |reason: String| ConfigError::Invalid { var, reason };
        let url: Url = raw
            .parse()
            .map_err(|e: url::ParseError| invalid(e.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(invalid("an http(s) URL naming a host".into()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(invalid(
                "a URL without credentials: the API token is the only credential (EP-67)".into(),
            ));
        }
        Ok(url)
    };
    let public = parse("JC_PORTAL_CKAN_URL", &public)?;
    if public.scheme() != "https" {
        return Err(ConfigError::Invalid {
            var: "JC_PORTAL_CKAN_URL",
            reason: "the public catalogue URL is https://, as every CkanInstance URL (EP-67)"
                .into(),
        });
    }
    let api = parse("JC_PORTAL_CKAN_API_URL", &api)?;
    let in_cluster = api
        .host_str()
        .is_some_and(|host| host.ends_with(".svc") || host.ends_with(".svc.cluster.local"));
    if api.scheme() != "https" && !in_cluster {
        return Err(ConfigError::Invalid {
            var: "JC_PORTAL_CKAN_API_URL",
            reason:
                "plain http only to a cluster Service (`<name>.<namespace>.svc.cluster.local`); \
                     anywhere else the token would cross the network in the clear"
                    .into(),
        });
    }
    Ok(Some((public, api)))
}

/// Where build pods run (AP-130): nothing when `JC_PORTAL_BUILD_NAMESPACE` is unset, and every
/// other part required once it is, because a namespace with no image would start pods that
/// never run.
fn build_pod_settings(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<crate::apps::build_pods::Settings>, ConfigError> {
    let set = |var: &str| {
        lookup(var)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let Some(namespace) = set("JC_PORTAL_BUILD_NAMESPACE") else {
        return Ok(None);
    };
    if !crate::resource::is_dns1123(&namespace) {
        return Err(ConfigError::Invalid {
            var: "JC_PORTAL_BUILD_NAMESPACE",
            reason: format!(
                "'{namespace}' is not a Kubernetes name (lowercase letters, digits, '-')"
            ),
        });
    }
    let image = |var: &'static str| match set(var) {
        Some(image) if image.contains("@sha256:") && !image.chars().any(char::is_whitespace) => {
            Ok(image)
        }
        Some(image) => Err(ConfigError::Invalid {
            var,
            reason: format!("'{image}' is not an image pinned by digest (name@sha256:…)"),
        }),
        None => Err(ConfigError::Invalid {
            var,
            reason: "JC_PORTAL_BUILD_NAMESPACE is set, and a build pod needs its image".into(),
        }),
    };
    let forge = set("JC_GITEA_URL").ok_or_else(|| ConfigError::Invalid {
        var: "JC_PORTAL_BUILD_NAMESPACE",
        reason: "a build pod registers with the forge, and JC_GITEA_URL is not set".into(),
    })?;
    let cache_size = set("JC_PORTAL_BUILD_CACHE_SIZE").unwrap_or_else(|| "1Gi".into());
    let digits = cache_size.trim_end_matches("Gi").trim_end_matches("Mi");
    if digits.is_empty()
        || digits.len() == cache_size.len()
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ConfigError::Invalid {
            var: "JC_PORTAL_BUILD_CACHE_SIZE",
            reason: format!("'{cache_size}' is not a size such as 512Mi or 1Gi"),
        });
    }
    Ok(Some(crate::apps::build_pods::Settings {
        namespace,
        forge,
        node_image: image("JC_PORTAL_BUILD_IMAGE_NODE")?,
        rust_image: image("JC_PORTAL_BUILD_IMAGE_RUST")?,
        cache_size,
    }))
}

/// Where an App's objects are applied: `JC_PORTAL_APPS_NAMESPACE` and `JC_PORTAL_ORG_DOMAIN`,
/// both or neither, with the host taken from the public URL rather than configured twice
/// (AP-13).
///
/// A pod-backed App also needs (AP-108):
///
/// - `JC_PORTAL_APPS_REGISTRY` — the host, and port if any, of the forge's container registry;
///   an App's image is composed as `{registry}/{forge organization}/app-{name}@{digest}`, the
///   organization being the applications' own when they have one of their own.
/// - `JC_GITEA_APPS_OWNER` — the forge organization the generated applications' repositories,
///   packages and images live in, apart from the configuration's (PF-106); the configuration's
///   organization when unset.
/// - `JC_GITEA_APPS_TOKEN` — the token of the applications' own machine user, which writes their
///   repositories and packages and nothing of the configuration's; set with the owner or not at
///   all.
/// - `JC_PORTAL_APPS_PULL_SECRET_NAME` — the name of the `dockerconfigjson` Secret in the apps
///   namespace a node pulls app images with (a forge token that reads packages only).
/// - `JC_PORTAL_APISIX_NAMESPACE` — the namespace the installation runs APISIX in, the only one
///   whose pods reach an app pod; default `apisix`.
/// - `JC_PORTAL_RELEASE` — the installation's release name; a project's pod-backed Apps run in
///   `{release}-{project}-apps`, bound to the ClusterRole `{release}-portal-apps` (AP-116).
/// - `JC_PORTAL_SERVICE_ACCOUNT` — the Portal's ServiceAccount in `JC_PORTAL_APPS_NAMESPACE`, the
///   subject of the RoleBinding in each project's apps namespace (AP-116).
fn app_settings(
    lookup: &impl Fn(&str) -> Option<String>,
    public_base_url: &Url,
    basemap: bool,
) -> Result<Option<crate::apps::reconciler::Settings>, ConfigError> {
    let set = |var: &str| {
        lookup(var)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (Some(namespace), Some(org_domain), Some(host)) = (
        set("JC_PORTAL_APPS_NAMESPACE"),
        set("JC_PORTAL_ORG_DOMAIN"),
        public_base_url.host_str().map(str::to_owned),
    ) else {
        return Ok(None);
    };
    let name = |var: &'static str, value: Option<String>| match value {
        Some(value) if !crate::resource::is_dns1123(&value) => Err(ConfigError::Invalid {
            var,
            reason: format!("'{value}' is not a Kubernetes name (lowercase letters, digits, '-')"),
        }),
        other => Ok(other),
    };
    let apisix_namespace = name(
        "JC_PORTAL_APISIX_NAMESPACE",
        set("JC_PORTAL_APISIX_NAMESPACE"),
    )?
    .unwrap_or_else(|| "apisix".to_owned());
    let pull_secret = name(
        "JC_PORTAL_APPS_PULL_SECRET_NAME",
        set("JC_PORTAL_APPS_PULL_SECRET_NAME"),
    )?;
    let image_repository = match set("JC_PORTAL_APPS_REGISTRY") {
        None => None,
        Some(registry) => {
            // A host and an optional port: a scheme or a path would compose a reference no
            // node resolves, and it is the one part of an image an App cannot name (AP-108).
            if !is_registry_host(&registry) {
                return Err(ConfigError::Invalid {
                    var: "JC_PORTAL_APPS_REGISTRY",
                    reason: format!(
                        "'{registry}' is not a registry host such as forge.example.org or \
                         forge.example.org:5000"
                    ),
                });
            }
            // The applications' organization when there is one (PF-106): their images are
            // published there, beside their repositories.
            let owner = set("JC_GITEA_APPS_OWNER")
                .or_else(|| set("JC_GITEA_OWNER"))
                .ok_or_else(|| ConfigError::Invalid {
                    var: "JC_PORTAL_APPS_REGISTRY",
                    reason: "the images live under the forge's organization, and JC_GITEA_OWNER \
                             is not set"
                        .to_owned(),
                })?;
            Some(format!("{registry}/{owner}"))
        }
    };
    let release = name("JC_PORTAL_RELEASE", set("JC_PORTAL_RELEASE"))?;
    let service_account = name(
        "JC_PORTAL_SERVICE_ACCOUNT",
        set("JC_PORTAL_SERVICE_ACCOUNT"),
    )?;
    let apex = apps_url(lookup)?
        .and_then(|apps| apps.host_str().map(str::to_owned))
        .unwrap_or_else(|| host.strip_prefix("portal.").unwrap_or(&host).to_owned());
    // Validated with the rest of the configuration where the Portal reads it itself.
    let gateway_url = set("JC_PORTAL_GATEWAY_URL").map(|url| url.trim_end_matches('/').to_owned());
    // The Portal's own public listener on its Service in the cluster, `http://portal.{namespace}
    // .svc.cluster.local:8080`: where an App pod asks `JC_ME_URL` for its caller's roles, because
    // the public host is outside the pod's NetworkPolicy (AP-109, AP-134). Without it no
    // pod-backed App is rendered, rather than one that reads every person as role-less.
    let service_url = match set("JC_PORTAL_SERVICE_URL") {
        None => None,
        Some(value) => {
            let parsed: Url = value
                .parse()
                .map_err(|e: url::ParseError| ConfigError::Invalid {
                    var: "JC_PORTAL_SERVICE_URL",
                    reason: e.to_string(),
                })?;
            let in_cluster = matches!(parsed.scheme(), "http" | "https")
                && parsed.path() == "/"
                && parsed.query().is_none()
                && parsed.host_str().is_some_and(|host| {
                    host.split('.')
                        .nth(1)
                        .is_some_and(crate::resource::is_dns1123)
                });
            if !in_cluster {
                return Err(ConfigError::Invalid {
                    var: "JC_PORTAL_SERVICE_URL",
                    reason: format!(
                        "'{value}' is not the Portal's Service in the cluster, such as \
                         http://portal.portal.svc.cluster.local:8080 (a scheme, \
                         {{service}}.{{namespace}} and a port, no path)"
                    ),
                });
            }
            Some(value.trim_end_matches('/').to_owned())
        }
    };
    Ok(Some(crate::apps::reconciler::Settings {
        host,
        apex,
        gateway_url,
        service_url,
        namespace,
        org_domain,
        apisix_namespace,
        image_repository,
        pull_secret,
        release,
        service_account,
        basemap_base: basemap.then(|| public_base_url.as_str().to_owned()),
    }))
}

/// The run's test sandbox (SDK-38): `JC_PORTAL_APP_TESTS_NAMESPACE`, a Kubernetes name, and
/// `JC_PORTAL_APP_TESTS_IMAGE`, an image reference pinned by `@sha256:` digest, both or neither;
/// with them, optionally `JC_PORTAL_APP_TESTS_RUST_IMAGE`, the rust runner pinned the same way,
/// which runs a `wasm` App's server tests (T-3618).
fn app_tests(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<crate::agents::sandbox::SandboxSettings>, ConfigError> {
    let set = |var: &str| {
        lookup(var)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    match (
        set("JC_PORTAL_APP_TESTS_NAMESPACE"),
        set("JC_PORTAL_APP_TESTS_IMAGE"),
    ) {
        (None, None) => Ok(None),
        (Some(namespace), Some(image)) => {
            if !crate::resource::is_dns1123(&namespace) {
                return Err(ConfigError::Invalid {
                    var: "JC_PORTAL_APP_TESTS_NAMESPACE",
                    reason: format!(
                        "'{namespace}' is not a Kubernetes name (lowercase letters, digits, '-')"
                    ),
                });
            }
            // The model's code runs on them, so they are the lane's own images and nothing a tag
            // could move (non-negotiable: images by digest).
            let pinned = |var: &'static str, image: String| {
                let digest = image.split_once("@sha256:").is_some_and(|(name, hex)| {
                    !name.is_empty()
                        && hex.len() == 64
                        && hex
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                });
                if digest && !image.chars().any(char::is_whitespace) {
                    return Ok(image);
                }
                Err(ConfigError::Invalid {
                    var,
                    reason: format!(
                        "'{image}' is not an image pinned by digest, such as \
                         ghcr.io/org/joinedcontext-app-builder@sha256:<64 hex>"
                    ),
                })
            };
            Ok(Some(crate::agents::sandbox::SandboxSettings {
                namespace,
                image: pinned("JC_PORTAL_APP_TESTS_IMAGE", image)?,
                rust_image: set("JC_PORTAL_APP_TESTS_RUST_IMAGE")
                    .map(|image| pinned("JC_PORTAL_APP_TESTS_RUST_IMAGE", image))
                    .transpose()?,
            }))
        }
        (Some(_), None) | (None, Some(_)) => Err(ConfigError::Invalid {
            var: "JC_PORTAL_APP_TESTS_NAMESPACE",
            reason: "JC_PORTAL_APP_TESTS_NAMESPACE and JC_PORTAL_APP_TESTS_IMAGE go together"
                .to_owned(),
        }),
    }
}

/// `forge.example.org` or `forge.example.org:5000`: lowercase DNS labels and an optional port.
fn is_registry_host(value: &str) -> bool {
    let (host, port_ok) = match value.split_once(':') {
        None => (value, true),
        Some((host, port)) => (host, port.parse::<u16>().is_ok_and(|port| port > 0)),
    };
    port_ok
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// Where a builder run happens and how the credential proxy reaches this Portal (ADR-N-020).
///
/// `JC_AGENTS_NAMESPACE` and `JC_AGENT_PROXY_BASE` are set together or not at all;
/// `JC_PORTAL_NAMESPACE` (default: the workspaces' own namespace), `JC_AGENT_RUN_TTL` (whole
/// seconds, default `1200`) and `JC_AGENT_APPROVAL_TTL` (whole seconds a run waits for its
/// change's approval, default `604800`) tune the rest. None of them is a secret: the proxy presents its own ServiceAccount
/// token, which is why `JC_AGENT_PROXY_TOKEN` is gone (T-2271).
///
/// The namespace and the proxy are needed together: a workspace with no proxy has no way to
/// reach the model, the data or the forge, and a proxy with no namespace has nothing to serve.
/// The token is the proxy's own credential on the internal listener, and it is the reason this
/// block is all-or-nothing rather than three independent variables: two of the three set is a
/// misconfiguration, not a Portal that runs agent runs unauthenticated.
#[derive(Clone)]
pub struct AgentSettings {
    /// Namespace the workspace Jobs, their ServiceAccounts and their NetworkPolicies go into.
    pub namespace: String,
    /// Namespace the Portal itself runs in, which is the only place a workspace's ingress rule
    /// admits from (AG-39). Unset means the Portal shares the workspaces' namespace, the same
    /// assumption the proxy's egress rule already makes.
    pub portal_namespace: String,
    /// Base URL of `jc-agent-proxy` as a workspace sees it, e.g.
    /// `http://jc-agent-proxy.agents.svc.cluster.local:8080`.
    pub proxy_base: String,
    /// Wall clock of one run, in seconds. The Job carries the same number as its
    /// `activeDeadlineSeconds`, so the two cannot disagree about when a run is over.
    pub run_ttl_secs: i64,
    /// How long a run that built an application waits for its change's approval, in seconds
    /// (`JC_AGENT_APPROVAL_TTL`, T-2772): the run's own wall clock stops when it proposes, and
    /// an approver has days, not what was left of the build's twenty minutes.
    pub approval_ttl_secs: i64,
}

impl std::fmt::Debug for AgentSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSettings")
            .field("namespace", &self.namespace)
            .field("portal_namespace", &self.portal_namespace)
            .field("proxy_base", &self.proxy_base)
            .field("run_ttl_secs", &self.run_ttl_secs)
            .field("approval_ttl_secs", &self.approval_ttl_secs)
            .finish()
    }
}

/// The agent runner block, or `None` when this Portal runs no agent (AG-33, AG-40).
fn agent_settings(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<AgentSettings>, ConfigError> {
    let namespace = lookup("JC_AGENTS_NAMESPACE").filter(|v| !v.trim().is_empty());
    let proxy_base = lookup("JC_AGENT_PROXY_BASE").filter(|v| !v.trim().is_empty());
    // `JC_AGENT_PROXY_TOKEN` was the third of these until T-2271: one string the Portal and the
    // proxy both held, which is the static key between cluster services CLAUDE.md rules out. The
    // proxy presents its own ServiceAccount token now (`JC_PORTAL_AGENT_PROXY_CLIENT_ID` says whose
    // token the callbacks accept), so there is no shared secret left to configure here.
    let (namespace, proxy_base) = match (namespace, proxy_base) {
        (None, None) => return Ok(None),
        (Some(namespace), Some(proxy_base)) => (namespace, proxy_base),
        _ => {
            return Err(ConfigError::Invalid {
                var: "JC_AGENTS_NAMESPACE",
                reason: "JC_AGENTS_NAMESPACE and JC_AGENT_PROXY_BASE must be set together"
                    .to_string(),
            })
        }
    };

    let probe: Url = proxy_base
        .parse()
        .map_err(|e: url::ParseError| ConfigError::Invalid {
            var: "JC_AGENT_PROXY_BASE",
            reason: e.to_string(),
        })?;
    if probe.scheme() != "http" && probe.scheme() != "https" {
        return Err(ConfigError::Invalid {
            var: "JC_AGENT_PROXY_BASE",
            reason: format!("scheme '{}' is not http or https", probe.scheme()),
        });
    }

    let run_ttl_secs = match lookup("JC_AGENT_RUN_TTL") {
        Some(value) => value.parse::<i64>().map_err(|e| ConfigError::Invalid {
            var: "JC_AGENT_RUN_TTL",
            reason: e.to_string(),
        })?,
        None => Config::DEFAULT_RUN_TTL_SECS,
    };
    if run_ttl_secs <= 0 {
        return Err(ConfigError::Invalid {
            var: "JC_AGENT_RUN_TTL",
            reason: "a run needs a positive wall clock".to_string(),
        });
    }

    let approval_ttl_secs = match lookup("JC_AGENT_APPROVAL_TTL") {
        Some(value) => value.parse::<i64>().map_err(|e| ConfigError::Invalid {
            var: "JC_AGENT_APPROVAL_TTL",
            reason: e.to_string(),
        })?,
        None => Config::DEFAULT_APPROVAL_TTL_SECS,
    };
    if approval_ttl_secs <= 0 {
        return Err(ConfigError::Invalid {
            var: "JC_AGENT_APPROVAL_TTL",
            reason: "a run waiting for approval needs a positive lease".to_string(),
        });
    }

    let portal_namespace = lookup("JC_PORTAL_NAMESPACE")
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| namespace.clone());

    Ok(Some(AgentSettings {
        namespace,
        portal_namespace,
        proxy_base: proxy_base.trim_end_matches('/').to_owned(),
        run_ttl_secs,
        approval_ttl_secs,
    }))
}

/// Configuration for the platform basemap proxy (AP-67).
///
/// `JC_BASEMAP_URL` (an `https` template holding `{z}`, `{x}` and `{y}`) turns the proxy on
/// and `JC_BASEMAP_ATTRIBUTION` is then required, because a tile source that is served without
/// its attribution is served against its licence. `JC_BASEMAP_MAX_ZOOM` (default `19`),
/// `JC_BASEMAP_CACHE_DIR` (default `/tmp/basemap-cache`), `JC_BASEMAP_CACHE_MAX_BYTES`
/// (default `268435456`) and `JC_BASEMAP_CACHE_TTL_SECS` (default `604800`) tune the cache.
/// `JC_BASEMAP_KEY_FILE` names a file holding the tile provider's key — a path to a secret,
/// and the reason the browser fetches tiles from the Portal rather than from the provider.
#[derive(Clone)]
pub struct BasemapConfig {
    pub url_template: String,
    pub attribution: String,
    pub max_zoom: u8,
    pub key: Option<String>,
    pub cache_dir: std::path::PathBuf,
    pub cache_max_bytes: u64,
    pub cache_ttl_secs: u64,
    pub allow_http: bool,
}

impl BasemapConfig {
    pub fn for_tests(
        url_template: String,
        attribution: String,
        cache_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            url_template,
            attribution,
            max_zoom: 19,
            key: None,
            cache_dir,
            cache_max_bytes: 1024 * 1024,
            cache_ttl_secs: 3600,
            allow_http: true,
        }
    }
}

impl std::fmt::Debug for BasemapConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasemapConfig")
            .field("url_template", &self.url_template)
            .field("attribution", &self.attribution)
            .field("max_zoom", &self.max_zoom)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("cache_dir", &self.cache_dir)
            .field("cache_max_bytes", &self.cache_max_bytes)
            .field("cache_ttl_secs", &self.cache_ttl_secs)
            .field("allow_http", &self.allow_http)
            .finish()
    }
}

/// `JC_PORTAL_APPS_STORE_ORIGIN`: an `https` origin and nothing more, or unset (T-3359). A path, a
/// query, a wildcard or plain `http` would widen every storing App's `connect-src` past one host.
fn apps_store_origin(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, ConfigError> {
    let Some(raw) = lookup("JC_PORTAL_APPS_STORE_ORIGIN").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let invalid = |reason: &str| ConfigError::Invalid {
        var: "JC_PORTAL_APPS_STORE_ORIGIN",
        reason: reason.to_owned(),
    };
    let url = Url::parse(raw.trim())
        .map_err(|_| invalid("not a URL; write the store's origin, `https://files.example.com`"))?;
    if url.scheme() != "https" {
        return Err(invalid("the store's origin is https"));
    }
    if url.host_str().is_none_or(|host| host.contains('*'))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("one host, no wildcard and no credentials"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("an origin alone, with no path, query or fragment"));
    }
    Ok(Some(url.origin().ascii_serialization()))
}

/// The basemap configuration block, or `None` when this Portal proxies no basemap (AP-67).
/// The operator's bounds file (PF-97). Absent, every entry keeps its built-in bound. A file that
/// cannot be read, is no map of catalog paths, holds a range upside down or loosens a security
/// bound stops the start: a Portal holding organizations to bounds nobody meant is worse than one
/// that says why it did not start.
fn organization_bounds(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<jc_core::kinds::OrganizationBounds, ConfigError> {
    const VAR: &str = "JC_PORTAL_ORGANIZATION_BOUNDS_FILE";
    let Some(path) = lookup(VAR).filter(|path| !path.trim().is_empty()) else {
        return Ok(jc_core::kinds::OrganizationBounds::default());
    };
    let invalid = |reason: String| ConfigError::Invalid { var: VAR, reason };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read '{path}': {e}")))?;
    // An empty file is a deployment that set no bound: the built-in ones.
    if text.trim().is_empty() {
        return Ok(jc_core::kinds::OrganizationBounds::default());
    }
    let bounds: jc_core::kinds::OrganizationBounds =
        serde_norway::from_str(&text).map_err(|e| {
            invalid(format!(
                "'{path}' is no map of catalog paths to {{min, max}}: {e}"
            ))
        })?;
    bounds
        .validate()
        .map_err(|e| invalid(format!("'{path}': {e}")))?;
    Ok(bounds)
}

/// The operator's price table (T-5126). Absent or empty, the Costs page says no table is set; a
/// file that cannot be read, does not parse or prices nothing stops the start, naming the field.
fn cost_prices(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<crate::costs::CostPrices>, ConfigError> {
    const VAR: &str = "JC_PORTAL_COST_PRICES_FILE";
    let Some(path) = lookup(VAR).filter(|path| !path.trim().is_empty()) else {
        return Ok(None);
    };
    let invalid = |reason: String| ConfigError::Invalid { var: VAR, reason };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read '{path}': {e}")))?;
    if text.trim().is_empty() || text.trim() == "{}" {
        return Ok(None);
    }
    let prices: crate::costs::CostPrices = serde_norway::from_str(&text)
        .map_err(|e| invalid(format!("'{path}' is no price table: {e}")))?;
    match prices.refusal() {
        Some(why) => Err(invalid(format!("'{path}': {why}"))),
        None => Ok(Some(prices)),
    }
}

fn basemap_config(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<BasemapConfig>, ConfigError> {
    let url_template = match lookup("JC_BASEMAP_URL").filter(|v| !v.trim().is_empty()) {
        Some(url) => url.trim().to_string(),
        None => return Ok(None),
    };

    if !url_template.contains("{z}")
        || !url_template.contains("{x}")
        || !url_template.contains("{y}")
    {
        return Err(ConfigError::Invalid {
            var: "JC_BASEMAP_URL",
            reason: "template must contain {z}, {x}, and {y}".to_string(),
        });
    }

    let dummy = url_template
        .replace("{z}", "0")
        .replace("{x}", "0")
        .replace("{y}", "0")
        .replace("{ext}", "png")
        .replace("{key}", "key");
    let probe: Url = dummy
        .parse()
        .map_err(|e: url::ParseError| ConfigError::Invalid {
            var: "JC_BASEMAP_URL",
            reason: e.to_string(),
        })?;
    if probe.scheme() != "https" {
        return Err(ConfigError::Invalid {
            var: "JC_BASEMAP_URL",
            reason: format!("scheme '{}' is not https", probe.scheme()),
        });
    }

    let attribution = match lookup("JC_BASEMAP_ATTRIBUTION").filter(|v| !v.trim().is_empty()) {
        Some(attr) => attr.trim().to_string(),
        None => {
            return Err(ConfigError::Invalid {
                var: "JC_BASEMAP_ATTRIBUTION",
                reason: "JC_BASEMAP_ATTRIBUTION is required when JC_BASEMAP_URL is set".to_string(),
            })
        }
    };

    let max_zoom = match lookup("JC_BASEMAP_MAX_ZOOM").filter(|v| !v.trim().is_empty()) {
        Some(val) => val.parse::<u8>().map_err(|e| ConfigError::Invalid {
            var: "JC_BASEMAP_MAX_ZOOM",
            reason: e.to_string(),
        })?,
        None => 19,
    };

    let key = match lookup("JC_BASEMAP_KEY_FILE").filter(|v| !v.trim().is_empty()) {
        Some(path) => {
            let content = std::fs::read_to_string(&path).map_err(|e| ConfigError::Invalid {
                var: "JC_BASEMAP_KEY_FILE",
                reason: format!("{path}: {e}"),
            })?;
            Some(content.trim().to_string())
        }
        None => None,
    };

    let cache_dir = std::path::PathBuf::from(
        lookup("JC_BASEMAP_CACHE_DIR")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "/tmp/basemap-cache".to_string()),
    );

    let cache_max_bytes =
        match lookup("JC_BASEMAP_CACHE_MAX_BYTES").filter(|v| !v.trim().is_empty()) {
            Some(val) => val.parse::<u64>().map_err(|e| ConfigError::Invalid {
                var: "JC_BASEMAP_CACHE_MAX_BYTES",
                reason: e.to_string(),
            })?,
            None => 268_435_456,
        };

    let cache_ttl_secs = match lookup("JC_BASEMAP_CACHE_TTL_SECS").filter(|v| !v.trim().is_empty())
    {
        Some(val) => val.parse::<u64>().map_err(|e| ConfigError::Invalid {
            var: "JC_BASEMAP_CACHE_TTL_SECS",
            reason: e.to_string(),
        })?,
        None => 604_800,
    };

    Ok(Some(BasemapConfig {
        url_template,
        attribution,
        max_zoom,
        key,
        cache_dir,
        cache_max_bytes,
        cache_ttl_secs,
        allow_http: false,
    }))
}

/// Keycloak realm the portal authenticates humans against (CC-40).
#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: Url,
    pub client_id: String,
    /// How the client proves itself at the token endpoint (PF-47).
    pub auth: ClientAuth,
    /// PEM of an extra root the discovery client trusts, on top of the compiled-in Mozilla
    /// bundle. An instance whose issuer is served by a private CA (a self-signed cluster
    /// issuer, an internal PKI) is unreachable without it: the binary carries `webpki-roots`
    /// alone, so no mounted file or `SSL_CERT_FILE` is consulted. Verification stays on;
    /// this only widens what a valid chain may end in.
    pub extra_ca_pem: Option<Vec<u8>>,
    /// Where the Portal itself reaches Keycloak for tokens and the admin API
    /// (`JC_KEYCLOAK_ADMIN_URL`, e.g. `http://keycloak.keycloak.svc:8080`; unset, the issuer's
    /// origin). In the cluster it is the Service, so the reconciler's calls do not pass the
    /// public edge and its per-address limit (T-4432). Tokens are still checked against the
    /// issuer.
    pub admin_url: Option<Url>,
}

impl OidcConfig {
    /// The realm URL the Portal posts to: the issuer's path on `admin_url`'s origin, or the
    /// issuer itself when it is unset. No trailing slash.
    pub fn backchannel(&self) -> String {
        let issuer = self.issuer.as_str().trim_end_matches('/');
        match &self.admin_url {
            None => issuer.to_owned(),
            Some(admin) => format!(
                "{}{}",
                admin.as_str().trim_end_matches('/'),
                self.issuer.path().trim_end_matches('/')
            ),
        }
    }
}

impl std::fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id)
            .field("auth", &self.auth)
            .field("admin_url", &self.admin_url.as_ref().map(Url::as_str))
            .finish()
    }
}

/// Errors raised when parsing configuration parameters.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("invalid configuration for {var}: {reason}")]
    Invalid { var: &'static str, reason: String },
}

impl Config {
    pub const DEFAULT_BIND: &'static str = "0.0.0.0:8080";
    const DEFAULT_BOOTSTRAP_ADMINS: &'static str = "platform-admins";
    pub const DEFAULT_PUBLIC_URL: &'static str = "http://localhost:8080";

    /// The data host Endpoint and MCP URLs are handed out on, `api.{domain}` beside
    /// `portal.{domain}` (ADR-N-052 §2.1, T-4632): an anonymous client on the Portal host is
    /// moved there. A Portal whose host is not `portal.…` has no data host of its own, so its
    /// own host serves.
    pub fn api_base_url(&self) -> Url {
        let mut url = self.public_base_url.clone();
        if let Some(domain) = url.host_str().and_then(|h| h.strip_prefix("portal.")) {
            let api = format!("api.{domain}");
            if url.set_host(Some(&api)).is_err() {
                return self.public_base_url.clone();
            }
        }
        url
    }
    /// The internal listener's default. Port 9090, which the edge does not route (AG-52).
    pub const DEFAULT_INTERNAL_BIND: &'static str = "0.0.0.0:9090";
    /// Wall clock of one builder run when the deployment names none: twenty minutes, the
    /// window AG-43 gives a run before it expires.
    pub const DEFAULT_RUN_TTL_SECS: i64 = 1_200;
    /// Seven days: a change proposed on Friday is still there to approve on Monday (T-2772).
    pub const DEFAULT_APPROVAL_TTL_SECS: i64 = 604_800;
    /// `Key::from` panics below 64 bytes, so the length is checked before it is called.
    pub const MIN_COOKIE_KEY_LEN: usize = 64;

    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind_str = lookup("JC_PORTAL_BIND").unwrap_or_else(|| Self::DEFAULT_BIND.to_string());
        let bind: SocketAddr =
            bind_str
                .parse()
                .map_err(|e: std::net::AddrParseError| ConfigError::Invalid {
                    var: "JC_PORTAL_BIND",
                    reason: e.to_string(),
                })?;
        let internal_bind: SocketAddr = lookup("JC_INTERNAL_BIND")
            .unwrap_or_else(|| Self::DEFAULT_INTERNAL_BIND.to_string())
            .parse()
            .map_err(|e: std::net::AddrParseError| ConfigError::Invalid {
                var: "JC_INTERNAL_BIND",
                reason: e.to_string(),
            })?;

        let url_str =
            lookup("JC_PORTAL_PUBLIC_URL").unwrap_or_else(|| Self::DEFAULT_PUBLIC_URL.to_string());
        let public_base_url: Url =
            url_str
                .parse()
                .map_err(|e: url::ParseError| ConfigError::Invalid {
                    var: "JC_PORTAL_PUBLIC_URL",
                    reason: e.to_string(),
                })?;

        let oidc = oidc_config(&lookup)?;

        let (cookie_key, cookie_keys_previous) = cookie_keys(&lookup, oidc.is_some())?;

        let sync_interval_secs = match lookup("JC_PORTAL_SYNC_INTERVAL") {
            Some(val) => val.parse::<u64>().map_err(|e| ConfigError::Invalid {
                var: "JC_PORTAL_SYNC_INTERVAL",
                reason: e.to_string(),
            })?,
            None => 60,
        };
        let sync_interval = Duration::from_secs(sync_interval_secs);

        let gitea_webhook_secret = lookup("JC_GITEA_WEBHOOK_SECRET");
        let gitea_webhook_secret_previous = lookup("JC_GITEA_WEBHOOK_SECRET_PREVIOUS");

        // The space surface's base: one host, no path. A typo here is a subscription that never
        // reaches its broker, so it is refused at startup like every other address.
        let gateway_url = http_url(&lookup, "JC_PORTAL_GATEWAY_URL")?
            .map(|url| url.trim_end_matches('/').to_owned());
        let gateway_pods_url = http_url(&lookup, "JC_PORTAL_GATEWAY_PODS_URL")?
            .map(|url| url.trim_end_matches('/').to_owned());

        // The broker's own address, checked the same way: a typo here is a registration the
        // hub never learns about, and a hub that federates nothing looks exactly like a hub
        // whose members are empty.
        let broker_url = http_url(&lookup, "JC_PORTAL_BROKER_URL")?
            .map(|url| url.trim_end_matches('/').to_owned());

        let broker_url_template = match lookup("JC_PORTAL_BROKER_URL_TEMPLATE")
            .filter(|value| !value.trim().is_empty())
        {
            Some(value) => Some(broker_url_template(&value)?),
            None => None,
        };

        let broker_admin_token_file = lookup("JC_PORTAL_BROKER_ADMIN_TOKEN_FILE")
            .filter(|path| !path.trim().is_empty())
            .map(std::path::PathBuf::from);

        // PL-19: a value nobody meant is refused at start-up, not read as either identity.
        let pipeline_identity = match lookup("JC_PORTAL_PIPELINE_IDENTITY")
            .as_deref()
            .map(str::trim)
        {
            None | Some("") | Some("project") => false,
            Some("pipeline") => true,
            Some(other) => {
                return Err(ConfigError::Invalid {
                    var: "JC_PORTAL_PIPELINE_IDENTITY",
                    reason: format!("`{other}` is neither `project` nor `pipeline`"),
                })
            }
        };

        // PL-78: a lane runs at least one pipeline at a time; anything else stops the start-up.
        let lane = |var: &'static str| match lookup(var).as_deref().map(str::trim) {
            None | Some("") => Ok(1),
            Some(text) => text
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| ConfigError::Invalid {
                    var,
                    reason: format!("`{text}` is not a whole number of at least 1"),
                }),
        };
        let pipelines_concurrency = (
            lane("JC_PORTAL_PIPELINES_BATCH_CONCURRENCY")?,
            lane("JC_PORTAL_PIPELINES_SHORT_CONCURRENCY")?,
        );

        let pipeline_namespace = dns_label(&lookup, "JC_PORTAL_PIPELINE_NAMESPACE")?;
        let app_identity_namespace = dns_label(&lookup, "JC_PORTAL_APP_IDENTITY_NAMESPACE")?;
        let mqtt_namespace = dns_label(&lookup, "JC_PORTAL_MQTT_NAMESPACE")?;
        let gateway_namespace = dns_label(&lookup, "JC_PORTAL_GATEWAY_NAMESPACE")?;
        if pipeline_identity && pipeline_namespace.is_none() {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_PIPELINE_IDENTITY",
                reason: "`pipeline` needs JC_PORTAL_PIPELINE_NAMESPACE, where the pipelines' \
                         accounts are made"
                    .to_owned(),
            });
        }

        // The template is not a URL until `{project}` is filled in, so it is checked against a
        // stand-in: an operator learns about a typo at startup, not on the first scrape.
        let pipeline_runner_url = lookup("JC_PORTAL_PIPELINE_RUNNER_URL")
            .map(|template| {
                http_scheme(
                    "JC_PORTAL_PIPELINE_RUNNER_URL",
                    &template.replace("{project}", "project"),
                )
                .map(|_| template)
            })
            .transpose()?;

        let pipeline_test_capture_url = http_url(&lookup, "JC_PORTAL_PIPELINE_TEST_CAPTURE_URL")?
            .map(|url| url.trim_end_matches('/').to_owned());

        let grafana = grafana_config(&lookup)?;
        let knowledge_url = http_url(&lookup, "JC_PORTAL_KNOWLEDGE_URL")?
            .map(|url| url.trim_end_matches('/').to_owned());

        let functions_url =
            http_url(&lookup, "JC_FUNCTIONS_URL")?.map(|url| url.trim_end_matches('/').to_owned());

        let model_tools_url = http_url(&lookup, "JC_PORTAL_MODEL_TOOLS_URL")?;
        let sdm_mirror_dir = lookup("JC_PORTAL_SDM_MIRROR_DIR").filter(|dir| !dir.is_empty());
        let ui_url =
            http_url(&lookup, "JC_PORTAL_UI_URL")?.map(|url| url.trim_end_matches('/').to_owned());

        // Only the literal `true` turns it on: a misspelling must not open the door (ADR-N-019).
        let trust_edge_token = lookup("JC_TRUST_EDGE_TOKEN").is_some_and(|v| v.trim() == "true");
        let connector_clients = connector_clients(lookup("JC_PORTAL_CONNECTOR_CLIENTS"));
        let dashboards = lookup("JC_PORTAL_DASHBOARDS").is_some_and(|v| v.trim() == "true");
        let setup = SetupStatements::from_vars(&lookup);

        let apps_dir = lookup("JC_PORTAL_APPS_DIR");
        let smtp_url = lookup("JC_PORTAL_SMTP_URL").filter(|url| !url.trim().is_empty());
        let erasure_key = lookup("JC_PORTAL_ERASURE_KEY").filter(|key| !key.trim().is_empty());
        let mail_from = match lookup("JC_PORTAL_MAIL_FROM").filter(|from| !from.trim().is_empty()) {
            Some(from) => Some(from.trim().parse::<lettre::Address>().map_err(|err| {
                ConfigError::Invalid {
                    var: "JC_PORTAL_MAIL_FROM",
                    reason: format!("not an email address: {err}"),
                }
            })?),
            None => None,
        };
        if smtp_url.is_some() && mail_from.is_none() {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_MAIL_FROM",
                reason: "JC_PORTAL_SMTP_URL is set, so mail needs an address to come from".into(),
            });
        }
        let apps_cache_dir =
            lookup("JC_PORTAL_APPS_CACHE_DIR").filter(|dir| !dir.trim().is_empty());
        let apps_url = apps_url(&lookup)?;
        let basemap = basemap_config(&lookup)?;
        let apps_store_origin = apps_store_origin(&lookup)?;
        let app_settings = app_settings(&lookup, &public_base_url, basemap.is_some())?;
        let build_pods = build_pod_settings(&lookup)?;
        let agent_settings = agent_settings(&lookup)?;
        let app_tests = app_tests(&lookup)?;
        let branding_file = lookup("JC_BRANDING_FILE").filter(|path| !path.trim().is_empty());
        let ckan_api_route = ckan_api_route(&lookup)?;
        let health_dir = lookup("JC_HEALTH_DIR").filter(|path| !path.trim().is_empty());
        let database_url = lookup("JC_PORTAL_DATABASE_URL").filter(|url| !url.trim().is_empty());
        let bootstrap_admins = lookup("JC_PORTAL_BOOTSTRAP_ADMINS")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| Self::DEFAULT_BOOTSTRAP_ADMINS.to_owned());
        let journey_users = lookup("JC_PORTAL_JOURNEY_USERS")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let decisions_need_second_factor = decisions_need_second_factor(&lookup);
        let keycloak_admin = keycloak_admin(&lookup, oidc.as_ref())?;

        let artifact_store = artifact_store_settings(&lookup);
        let apps_db = apps_db_settings(&lookup)?;
        let pipeline_secrets = pipeline_secret_backend(&lookup);

        Ok(Self {
            bind,
            internal_bind,
            public_base_url,
            oidc,
            trust_edge_token,
            connector_clients,
            dashboards,
            setup,
            cookie_key,
            cookie_keys_previous,
            sync_interval,
            gitea_webhook_secret,
            gitea_webhook_secret_previous,
            pipeline_runner_url,
            pipeline_identity,
            pipelines_concurrency,
            pipeline_namespace,
            app_identity_namespace,
            mqtt_namespace,
            gateway_namespace,
            gateway_url,
            gateway_pods_url,
            gateway_client_id: lookup("JC_PORTAL_GATEWAY_CLIENT_ID")
                .filter(|v| !v.trim().is_empty()),
            agent_proxy_client_id: lookup("JC_PORTAL_AGENT_PROXY_CLIENT_ID")
                .filter(|v| !v.trim().is_empty()),
            pipeline_runner_client_id: lookup("JC_PORTAL_PIPELINE_RUNNER_CLIENT_ID")
                .filter(|v| !v.trim().is_empty()),
            invitation_client_id: lookup("JC_PORTAL_INVITATION_CLIENT_ID")
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
            broker_url,
            broker_url_template,
            broker_admin_token_file,
            org_domain: lookup("JC_PORTAL_ORG_DOMAIN").filter(|v| !v.trim().is_empty()),
            pipeline_test_capture_url,
            model_tools_url,
            sdm_mirror_dir,
            ui_url,
            functions_url,
            knowledge_url,
            grafana,
            apps_dir,
            smtp_url,
            erasure_key,
            mail_from,
            apps_cache_dir,
            apps_url,
            branding_file,
            ckan_api_route,
            health_dir,
            database_url,
            bootstrap_admins,
            decisions_need_second_factor,
            journey_users,
            keycloak_admin,
            organization_bounds: organization_bounds(&lookup)?,
            cost_prices: cost_prices(&lookup)?,
            app_settings,
            build_pods,
            agent_settings,
            app_tests,
            basemap,
            apps_store_origin,
            artifact_store,
            apps_db,
            pipeline_secrets,
        })
    }

    pub fn for_tests() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            internal_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            public_base_url: Url::parse("http://localhost:8080")
                .unwrap_or_else(|_| unreachable!("valid test url")),
            oidc: None,
            trust_edge_token: false,
            connector_clients: connector_clients(None),
            // The tests exercise the section; hiding it is tested where it is decided.
            dashboards: true,
            setup: SetupStatements::default(),
            artifact_store: None,
            apps_db: None,
            pipeline_secrets: None,
            cookie_key: Key::generate(),
            cookie_keys_previous: Vec::new(),
            sync_interval: Duration::ZERO,
            gitea_webhook_secret: None,
            gitea_webhook_secret_previous: None,
            pipeline_runner_url: None,
            pipeline_identity: false,
            pipelines_concurrency: (1, 1),
            pipeline_namespace: None,
            app_identity_namespace: None,
            mqtt_namespace: None,
            gateway_namespace: None,
            gateway_url: None,
            gateway_pods_url: None,
            gateway_client_id: None,
            agent_proxy_client_id: None,
            pipeline_runner_client_id: None,
            invitation_client_id: None,
            broker_url: None,
            broker_url_template: None,
            broker_admin_token_file: None,
            org_domain: None,
            pipeline_test_capture_url: None,
            model_tools_url: None,
            sdm_mirror_dir: None,
            ui_url: None,
            functions_url: None,
            knowledge_url: None,
            grafana: None,
            app_settings: None,
            build_pods: None,
            agent_settings: None,
            app_tests: None,
            basemap: None,
            apps_store_origin: None,
            apps_dir: None,
            smtp_url: None,
            erasure_key: None,
            mail_from: None,
            apps_cache_dir: None,
            apps_url: None,
            branding_file: None,
            ckan_api_route: None,
            health_dir: None,
            database_url: None,
            // The dev realm's approver role: a test session that carries it may do everything,
            // one that does not is bound by whatever Role/RoleBinding the test puts in the mirror.
            bootstrap_admins: "portal-approver".to_owned(),
            decisions_need_second_factor: false,
            journey_users: Vec::new(),
            keycloak_admin: None,
            organization_bounds: jc_core::kinds::OrganizationBounds::default(),
            cost_prices: None,
        }
    }

    /// Redirect URI registered for this portal in the Keycloak client (CC-40).
    pub fn redirect_uri(&self) -> String {
        format!(
            "{}/api/v1/auth/callback",
            self.public_base_url.as_str().trim_end_matches('/')
        )
    }
}

/// The configuration reference is generated from the doc comments of this module, so a
/// variable read without one is caught here rather than in the docs lane (T-2140, OPS-27).
#[cfg(test)]
#[path = "config_documentation_tests.rs"]
mod config_documentation_tests;

/// `JC_PORTAL_BROKER_URL_TEMPLATE`: an http(s) scheme and an authority holding `{broker}`, no path.
fn broker_url_template(raw: &str) -> Result<String, ConfigError> {
    let invalid = |reason: &str| ConfigError::Invalid {
        var: "JC_PORTAL_BROKER_URL_TEMPLATE",
        reason: reason.to_owned(),
    };
    let raw = raw.trim().trim_end_matches('/');
    let sample: Url = raw
        .replace("{broker}", "b")
        .parse()
        .map_err(|_| invalid("is not a URL"))?;
    if !raw.contains("{broker}")
        || raw
            .split("://")
            .nth(1)
            .is_some_and(|rest| rest.contains('/'))
    {
        return Err(invalid("must name {broker} in the authority, with no path"));
    }
    if sample.scheme() != "http" && sample.scheme() != "https" {
        return Err(invalid("must be http or https"));
    }
    Ok(raw.to_owned())
}

/// The Keycloak clients of the MCP connectors the platform ships (deployment
/// components/context-gateway/keycloak-clients.yaml): the person's connector on a phone or the
/// web (`mcp-mobile`), on their computer (`mcp-local`, Claude Code and IDEs) and the hub's.
/// The `edge` client also holds `mcp:portal`, but its token is the Portal login itself.
pub const CONNECTOR_CLIENTS: [&str; 3] = ["mcp-mobile", "mcp-hub", "mcp-local"];

/// [`CONNECTOR_CLIENTS`] and the names `extra` adds, each once.
fn connector_clients(extra: Option<String>) -> Vec<String> {
    let mut clients: Vec<String> = CONNECTOR_CLIENTS.map(String::from).to_vec();
    for name in extra.iter().flat_map(|v| v.split(',')).map(str::trim) {
        if !name.is_empty() && !clients.iter().any(|c| c == name) {
            clients.push(name.to_owned());
        }
    }
    clients
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(var: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |name: &str| (name == var).then(|| value.to_owned())
    }

    /// T-5314: the catalogue's API moves to its Service only with both URLs, the public one https,
    /// and plain http only to a cluster Service name; each refusal names the variable to fix.
    #[test]
    fn the_catalogue_api_route_needs_both_urls_and_http_only_inside_the_cluster() {
        let route = |public: Option<&'static str>, api: Option<&'static str>| {
            ckan_api_route(&move |name: &str| match name {
                "JC_PORTAL_CKAN_URL" => public.map(str::to_owned),
                "JC_PORTAL_CKAN_API_URL" => api.map(str::to_owned),
                _ => None,
            })
        };
        assert_eq!(route(None, None).unwrap(), None);
        assert_eq!(route(Some(" "), Some("")).unwrap(), None);
        let (public, api) = route(
            Some("https://opendata.example.org"),
            Some("http://ckan.ckan.svc.cluster.local:5000"),
        )
        .unwrap()
        .expect("a route");
        assert_eq!(public.as_str(), "https://opendata.example.org/");
        assert_eq!(api.as_str(), "http://ckan.ckan.svc.cluster.local:5000/");
        assert!(route(
            Some("https://o.example.org"),
            Some("https://ckan.internal.example.org")
        )
        .unwrap()
        .is_some());
        for (public, api, named) in [
            (
                None,
                Some("http://ckan.ckan.svc:5000"),
                "JC_PORTAL_CKAN_URL",
            ),
            (
                Some("https://o.example.org"),
                None,
                "JC_PORTAL_CKAN_API_URL",
            ),
            (
                Some("http://o.example.org"),
                Some("http://ckan.ckan.svc:5000"),
                "JC_PORTAL_CKAN_URL",
            ),
            (
                Some("https://o.example.org"),
                Some("http://ckan.example.org:5000"),
                "JC_PORTAL_CKAN_API_URL",
            ),
            (
                Some("https://o.example.org"),
                Some("http://svc.example.org"),
                "JC_PORTAL_CKAN_API_URL",
            ),
            (
                Some("https://o.example.org"),
                Some("http://u:p@ckan.ckan.svc:5000"),
                "JC_PORTAL_CKAN_API_URL",
            ),
            (
                Some("https://o.example.org"),
                Some("ftp://ckan.ckan.svc"),
                "JC_PORTAL_CKAN_API_URL",
            ),
            (
                Some("opendata"),
                Some("http://ckan.ckan.svc:5000"),
                "JC_PORTAL_CKAN_URL",
            ),
        ] {
            match route(public, api) {
                Err(ConfigError::Invalid { var, .. }) => {
                    assert_eq!(var, named, "{public:?} {api:?}")
                }
                other => panic!("{public:?} {api:?}: {other:?}"),
            }
        }
    }

    /// T-4470: the parts `Config::from_vars` was cut into, each on its own input.
    #[test]
    fn an_address_is_http_or_https_or_refused_by_name() {
        let var = "JC_PORTAL_KNOWLEDGE_URL";
        assert_eq!(
            http_url(&one(var, "http://k:80/"), var).unwrap(),
            Some("http://k:80/".into())
        );
        assert_eq!(http_url(&one("OTHER", "x"), var).unwrap(), None);
        for bad in ["ftp://k", "not a url", ""] {
            match http_url(&one(var, bad), var) {
                Err(ConfigError::Invalid { var: named, .. }) => assert_eq!(named, var, "{bad}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
    }

    /// T-6083: the decision doors' second-factor gate stays on unless the key is exactly `false`.
    #[test]
    fn only_the_literal_false_turns_the_decision_second_factor_off() {
        let with = |value: Option<&'static str>| {
            Config::from_vars(move |name| {
                (name == "JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR")
                    .then_some(value)
                    .flatten()
                    .map(str::to_owned)
            })
            .expect("valid")
            .decisions_need_second_factor
        };
        assert!(with(None), "unset keeps it on");
        assert!(!with(Some("false")));
        for kept_on in ["true", "0", "no", "False", " false", "off", ""] {
            assert!(with(Some(kept_on)), "{kept_on:?} keeps it on");
        }
    }

    /// T-4470 edge cases: a trailing slash goes from the addresses paths are appended to and stays
    /// on the model tools; the runner URL is a template checked with `{project}` filled in.
    #[test]
    fn addresses_keep_or_drop_their_slash_and_the_runner_template_is_probed() {
        let vars = |name: &str| match name {
            "JC_PORTAL_GATEWAY_URL" => Some("http://gw:8080/".to_owned()),
            "JC_PORTAL_GATEWAY_PODS_URL" => Some("http://gw-pods:8080/".to_owned()),
            "JC_PORTAL_MODEL_TOOLS_URL" => Some("http://tools:8000/".to_owned()),
            "JC_PORTAL_UI_URL" => Some("http://portal-ui:8080/".to_owned()),
            "JC_PORTAL_PIPELINE_RUNNER_URL" => Some("http://runner.{project}.svc:4195".to_owned()),
            _ => None,
        };
        let config = Config::from_vars(vars).expect("valid");
        assert_eq!(config.gateway_url.as_deref(), Some("http://gw:8080"));
        assert_eq!(
            config.gateway_pods_url.as_deref(),
            Some("http://gw-pods:8080")
        );
        assert_eq!(config.ui_url.as_deref(), Some("http://portal-ui:8080"));
        assert_eq!(
            config.model_tools_url.as_deref(),
            Some("http://tools:8000/")
        );
        assert_eq!(
            config.pipeline_runner_url.as_deref(),
            Some("http://runner.{project}.svc:4195"),
            "the template is kept as written"
        );
        let bad_runner = |name: &str| {
            (name == "JC_PORTAL_PIPELINE_RUNNER_URL").then(|| "ftp://runner.{project}".to_owned())
        };
        match Config::from_vars(bad_runner) {
            Err(ConfigError::Invalid { var, .. }) => {
                assert_eq!(var, "JC_PORTAL_PIPELINE_RUNNER_URL")
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn a_namespace_is_a_dns_label_and_blank_is_unset() {
        let var = "JC_PORTAL_MQTT_NAMESPACE";
        assert_eq!(
            dns_label(&one(var, " mqtt "), var).unwrap(),
            Some("mqtt".into())
        );
        assert_eq!(dns_label(&one(var, "  "), var).unwrap(), None);
        assert!(dns_label(&one(var, "Not_A_Label"), var).is_err());
    }

    #[test]
    fn the_login_is_all_three_or_nothing() {
        assert!(oidc_config(&|_: &str| None).unwrap().is_none());
        assert!(oidc_config(&one("JC_OIDC_ISSUER", "https://kc/realms/x")).is_err());
        let full = |name: &str| match name {
            "JC_OIDC_ISSUER" => Some("https://kc/realms/x".to_owned()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_owned()),
            "JC_OIDC_CLIENT_SECRET" => Some("s".to_owned()),
            "JC_KEYCLOAK_ADMIN_URL" => Some("http://kc:8080/admin".to_owned()),
            _ => None,
        };
        // An admin URL with a path is refused: it would be pasted into every admin call.
        assert!(oidc_config(&full).is_err());
        let origin = |name: &str| match name {
            "JC_KEYCLOAK_ADMIN_URL" => Some("http://kc:8080".to_owned()),
            other => full(other),
        };
        let oidc = oidc_config(&origin).unwrap().expect("configured");
        assert_eq!(oidc.client_id, "portal");
        assert_eq!(
            oidc.admin_url.map(String::from),
            Some("http://kc:8080/".to_owned())
        );
    }

    #[test]
    fn cookie_keys_are_long_enough_or_refused() {
        let long = "k".repeat(Config::MIN_COOKIE_KEY_LEN);
        let (_, previous) = cookie_keys(&|_: &str| None, false).unwrap();
        assert!(previous.is_empty());
        assert!(
            cookie_keys(&|_: &str| None, true).is_err(),
            "a Portal people sign in to has no ephemeral key"
        );
        let rotating = |name: &str| match name {
            "JC_PORTAL_COOKIE_KEY" => Some(long.clone()),
            "JC_PORTAL_COOKIE_KEY_PREVIOUS" => Some(format!("{long}, ,{long}")),
            _ => None,
        };
        assert_eq!(cookie_keys(&rotating, true).unwrap().1.len(), 2);
        assert!(cookie_keys(&one("JC_PORTAL_COOKIE_KEY", "short"), false).is_err());
        assert!(cookie_keys(&one("JC_PORTAL_COOKIE_KEY_PREVIOUS", "short"), false).is_err());
    }

    #[test]
    fn the_admin_client_needs_an_id_and_exactly_one_proof() {
        assert!(keycloak_admin(&|_: &str| None, None).unwrap().is_none());
        // An id without a proof would log a refusal every tick.
        assert!(
            keycloak_admin(&one("JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID", "groups"), None).is_err()
        );
        // A minted token's audience is the realm: no issuer, no ServiceAccount proof.
        let account = one("JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT", "portal");
        assert!(keycloak_admin(&account, None).is_err());
        let both = |name: &str| match name {
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET" => Some("s".to_owned()),
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT" => Some("portal".to_owned()),
            _ => None,
        };
        assert!(keycloak_admin(&both, None).is_err());
        let secret = |name: &str| match name {
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID" => Some("groups".to_owned()),
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET" => Some("s".to_owned()),
            _ => None,
        };
        let (id, _) = keycloak_admin(&secret, None).unwrap().expect("configured");
        assert_eq!(id, "groups");
    }

    /// T-2772: a run waiting for approval has seven days unless `JC_AGENT_APPROVAL_TTL` says
    /// otherwise, and a lease that is not a positive number of seconds is refused at start.
    #[test]
    fn the_approval_lease_defaults_to_a_week_and_refuses_nonsense() {
        let with = |ttl: Option<&'static str>| {
            agent_settings(&move |name: &str| match name {
                "JC_AGENTS_NAMESPACE" => Some("agents".to_owned()),
                "JC_AGENT_PROXY_BASE" => Some("http://proxy:8080".to_owned()),
                "JC_AGENT_APPROVAL_TTL" => ttl.map(str::to_owned),
                _ => None,
            })
        };
        let settings = |ttl| with(ttl).ok().flatten().map(|s| s.approval_ttl_secs);
        assert_eq!(settings(None), Some(604_800));
        assert_eq!(settings(Some("172800")), Some(172_800));
        for bad in ["0", "-5", "a week"] {
            assert!(with(Some(bad)).is_err(), "{bad}");
        }
    }

    /// T-2716 (PF-97): the operator's bounds file; none keeps the built-in bounds, and a file
    /// that loosens a security bound, names no catalog path or cannot be read stops the start.
    #[test]
    fn the_organization_bounds_are_the_operators_file_or_the_built_in_ones() {
        let dir = std::env::temp_dir().join(format!("jc-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let file = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text).expect("the bounds file");
            path.to_string_lossy().into_owned()
        };
        let with = |path: Option<String>| {
            organization_bounds(&move |name: &str| {
                (name == "JC_PORTAL_ORGANIZATION_BOUNDS_FILE")
                    .then(|| path.clone())
                    .flatten()
            })
        };
        assert_eq!(with(None).expect("no file"), Default::default());
        assert_eq!(
            with(Some(file("empty.yaml", "\n"))).expect("empty"),
            Default::default()
        );

        let tight = with(Some(file(
            "tight.yaml",
            "spec.projects.quota.contextSpaces: { max: 20 }\n\
             spec.limits.signIn.sessionIdleMinutes: { max: 120 }\n",
        )))
        .expect("a tighter security bound and a quota ceiling are the operator's to set");
        let quota = jc_core::kinds::org_settings::entry_at("spec.projects.quota.contextSpaces")
            .expect("a catalog entry");
        assert_eq!(tight.range(quota), (0, Some(20)));

        for (name, text) in [
            (
                "loose.yaml",
                "spec.limits.signIn.sessionIdleMinutes: { max: 600 }\n",
            ),
            ("unknown.yaml", "spec.limits.nothing: { max: 1 }\n"),
            (
                "upside.yaml",
                "spec.projects.quota.apps: { min: 5, max: 1 }\n",
            ),
            ("typo.yaml", "spec.projects.quota.apps: { maximum: 5 }\n"),
        ] {
            let refused = with(Some(file(name, text))).expect_err(name).to_string();
            assert!(
                refused.contains("JC_PORTAL_ORGANIZATION_BOUNDS_FILE"),
                "{refused}"
            );
        }
        let missing = dir.join("absent.yaml").to_string_lossy().into_owned();
        assert!(with(Some(missing)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broker_template_names_the_broker_in_its_authority_and_nothing_else() {
        let with = |value: &'static str| {
            Config::from_vars(move |name| {
                (name == "JC_PORTAL_BROKER_URL_TEMPLATE").then(|| value.to_owned())
            })
        };
        assert_eq!(
            with("http://broker-{broker}.ns.svc:8080/")
                .unwrap()
                .broker_url_template,
            Some("http://broker-{broker}.ns.svc:8080".to_owned())
        );
        assert_eq!(with(" ").unwrap().broker_url_template, None);
        for refused in [
            "http://broker.ns.svc",
            "http://broker.ns.svc/{broker}",
            "ftp://{broker}.ns.svc",
            "{broker}",
        ] {
            assert!(with(refused).is_err(), "{refused} was accepted");
        }
    }

    #[test]
    fn the_test_sandbox_is_a_namespace_and_an_image_pinned_by_digest_or_nothing() {
        let with = |namespace: &'static str, image: &'static str| {
            Config::from_vars(move |name| match name {
                "JC_PORTAL_APP_TESTS_NAMESPACE" => Some(namespace.to_owned()),
                "JC_PORTAL_APP_TESTS_IMAGE" => Some(image.to_owned()),
                _ => None,
            })
        };
        let digest = "a".repeat(64);
        let image: &'static str = Box::leak(
            format!("ghcr.io/x/joinedcontext-app-builder:main@sha256:{digest}").into_boxed_str(),
        );
        let settings = with("dev-app-tests", image).unwrap().app_tests.unwrap();
        assert_eq!(settings.namespace, "dev-app-tests");
        assert_eq!(settings.image, image);
        assert_eq!(Config::from_vars(|_| None).unwrap().app_tests, None);
        assert_eq!(with(" ", " ").unwrap().app_tests, None);
        for (namespace, image) in [
            ("dev-app-tests", "ghcr.io/x/joinedcontext-app-builder:main"),
            ("dev-app-tests", "ghcr.io/x/builder@sha256:ABC"),
            ("Dev_Tests", image),
            ("dev-app-tests", ""),
            ("", image),
        ] {
            assert!(
                with(namespace, image).is_err(),
                "{namespace} {image} was accepted"
            );
        }
    }

    /// T-3618: the rust runner of a `wasm` App's server tests is optional and pinned by digest
    /// like the lane's image; it means nothing without the sandbox.
    #[test]
    fn the_sandbox_s_rust_runner_is_an_image_pinned_by_digest_or_nothing() {
        let digest = "b".repeat(64);
        let node = format!(
            "ghcr.io/x/joinedcontext-app-builder@sha256:{}",
            "a".repeat(64)
        );
        let rust = format!("ghcr.io/x/joinedcontext-app-builder:rust@sha256:{digest}");
        let with = |sandbox: bool, rust: &str| {
            let (node, rust) = (node.clone(), rust.to_owned());
            Config::from_vars(move |name| match name {
                "JC_PORTAL_APP_TESTS_NAMESPACE" if sandbox => Some("dev-app-tests".to_owned()),
                "JC_PORTAL_APP_TESTS_IMAGE" if sandbox => Some(node.clone()),
                "JC_PORTAL_APP_TESTS_RUST_IMAGE" => Some(rust.clone()),
                _ => None,
            })
        };
        let settings = with(true, &rust).unwrap().app_tests.unwrap();
        assert_eq!(settings.rust_image.as_deref(), Some(rust.as_str()));
        assert_eq!(settings.image, node);
        assert_eq!(with(true, " ").unwrap().app_tests.unwrap().rust_image, None);
        assert_eq!(with(false, &rust).unwrap().app_tests, None);
        for bad in ["ghcr.io/x/builder:rust", "ghcr.io/x/builder@sha256:ABC"] {
            let Err(err) = with(true, bad) else {
                panic!("{bad} was accepted");
            };
            assert!(
                err.to_string().contains("JC_PORTAL_APP_TESTS_RUST_IMAGE"),
                "{err}"
            );
        }
    }

    #[test]
    fn the_apps_origin_is_an_origin_and_nothing_else() {
        let with = |value: &'static str| {
            Config::from_vars(move |name| (name == "JC_PORTAL_APPS_URL").then(|| value.to_owned()))
        };
        assert_eq!(Config::from_vars(|_| None).unwrap().apps_url, None);
        assert_eq!(with("  ").unwrap().apps_url, None);
        assert_eq!(
            with("https://example.org")
                .unwrap()
                .apps_url
                .unwrap()
                .as_str(),
            "https://example.org/"
        );
        for refused in [
            "example.org",
            "ftp://example.org",
            "https://example.org/apps",
            "https://example.org/?x=1",
        ] {
            assert!(
                matches!(
                    with(refused),
                    Err(ConfigError::Invalid {
                        var: "JC_PORTAL_APPS_URL",
                        ..
                    })
                ),
                "{refused}"
            );
        }
    }

    #[test]
    fn default_values() {
        let config = Config::from_vars(|_| None).expect("default config");
        assert_eq!(
            config.bind,
            "0.0.0.0:8080".parse().expect("parse default bind")
        );
        assert_eq!(config.public_base_url.as_str(), "http://localhost:8080/");
    }

    /// PL-19 (T-1508): streams keep the project's account until the switch says `pipeline`, and a
    /// value nobody meant stops the Portal at start-up instead of picking either identity.
    #[test]
    fn the_pipeline_identity_is_the_projects_until_switched_and_nothing_else_is_read() {
        let with = |value: Option<&str>| {
            Config::from_vars(|k| match k {
                "JC_PORTAL_PIPELINE_IDENTITY" => value.map(str::to_owned),
                "JC_PORTAL_PIPELINE_NAMESPACE" => Some("jc-pipeline-identities".to_owned()),
                _ => None,
            })
        };
        assert!(!with(None).expect("default").pipeline_identity);
        assert!(!with(Some("project")).expect("project").pipeline_identity);
        assert!(with(Some("pipeline")).expect("pipeline").pipeline_identity);
        let err = with(Some("pipelines"))
            .expect_err("a typo is refused")
            .to_string();
        assert!(err.contains("JC_PORTAL_PIPELINE_IDENTITY"), "{err}");
    }

    /// PL-78: each lane runs one pipeline at a time until configured, and a count below one or a
    /// word stops the start-up.
    #[test]
    fn each_pipeline_lane_runs_one_at_a_time_unless_configured() {
        let with = |batch: Option<&str>, short: Option<&str>| {
            Config::from_vars(|k| match k {
                "JC_PORTAL_PIPELINES_BATCH_CONCURRENCY" => batch.map(str::to_owned),
                "JC_PORTAL_PIPELINES_SHORT_CONCURRENCY" => short.map(str::to_owned),
                _ => None,
            })
        };
        assert_eq!(
            with(None, None).expect("default").pipelines_concurrency,
            (1, 1)
        );
        assert_eq!(
            with(Some("2"), Some(" 3 "))
                .expect("set")
                .pipelines_concurrency,
            (2, 3)
        );
        for bad in ["0", "-1", "two"] {
            let err = with(None, Some(bad)).expect_err(bad).to_string();
            assert!(
                err.contains("JC_PORTAL_PIPELINES_SHORT_CONCURRENCY"),
                "{err}"
            );
        }
    }

    /// T-1508: the switch needs the namespace its accounts are made in, and the namespace is a
    /// DNS-1123 label or the Portal does not start.
    #[test]
    fn the_pipeline_switch_needs_a_valid_namespace() {
        let with = |identity: &str, namespace: Option<&str>| {
            Config::from_vars(|k| match k {
                "JC_PORTAL_PIPELINE_IDENTITY" => Some(identity.to_owned()),
                "JC_PORTAL_PIPELINE_NAMESPACE" => namespace.map(str::to_owned),
                _ => None,
            })
        };
        let err = with("pipeline", None)
            .expect_err("no namespace")
            .to_string();
        assert!(err.contains("JC_PORTAL_PIPELINE_NAMESPACE"), "{err}");
        let err = with("project", Some("Not_A_Label"))
            .expect_err("bad namespace")
            .to_string();
        assert!(err.contains("JC_PORTAL_PIPELINE_NAMESPACE"), "{err}");
        let config = with("project", Some("jc-pipeline-identities")).expect("dormant");
        assert!(!config.pipeline_identity);
        assert_eq!(
            config.pipeline_namespace.as_deref(),
            Some("jc-pipeline-identities")
        );
    }

    /// T-3994: the MQTT brokers' namespace is read when set, absent when blank, and refused when it
    /// is not a DNS-1123 label.
    #[test]
    fn the_mqtt_namespace_is_a_dns_label_or_absent() {
        let with = |value: Option<&str>| {
            Config::from_vars(|k| match k {
                "JC_PORTAL_MQTT_NAMESPACE" => value.map(str::to_owned),
                _ => None,
            })
        };
        assert_eq!(with(None).expect("unset").mqtt_namespace, None);
        assert_eq!(with(Some(" ")).expect("blank").mqtt_namespace, None);
        assert_eq!(
            with(Some("jc-mqtt"))
                .expect("set")
                .mqtt_namespace
                .as_deref(),
            Some("jc-mqtt")
        );
        assert!(with(Some("JC_Mqtt")).is_err());
    }

    /// AP-159 (T-3539): the Apps' identities namespace is read when set, absent when blank, and a
    /// value that is not a DNS-1123 label stops the Portal at start-up.
    #[test]
    fn the_app_identity_namespace_is_a_dns_label_or_absent() {
        let with = |value: Option<&str>| {
            Config::from_vars(|k| match k {
                "JC_PORTAL_APP_IDENTITY_NAMESPACE" => value.map(str::to_owned),
                _ => None,
            })
        };
        assert_eq!(with(None).expect("unset").app_identity_namespace, None);
        assert_eq!(
            with(Some("  ")).expect("blank").app_identity_namespace,
            None
        );
        assert_eq!(
            with(Some("jc-app-identities"))
                .expect("set")
                .app_identity_namespace
                .as_deref(),
            Some("jc-app-identities")
        );
        let err = with(Some("Not_A_Label"))
            .expect_err("bad namespace")
            .to_string();
        assert!(err.contains("JC_PORTAL_APP_IDENTITY_NAMESPACE"), "{err}");
    }

    #[test]
    fn custom_valid_values() {
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_BIND" => Some("127.0.0.1:9090".to_string()),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com".to_string()),
            _ => None,
        })
        .expect("custom config");
        assert_eq!(
            config.bind,
            "127.0.0.1:9090".parse().expect("parse custom bind")
        );
        assert_eq!(
            config.public_base_url.as_str(),
            "https://portal.example.com/"
        );
    }

    #[test]
    fn invalid_bind_produces_error() {
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_BIND" => Some("invalid-bind".to_string()),
            _ => None,
        })
        .expect_err("should fail with invalid bind");

        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_BIND"),
        }
    }

    #[test]
    fn invalid_public_url_produces_error() {
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("not a valid url".to_string()),
            _ => None,
        })
        .expect_err("should fail with invalid url");

        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_PUBLIC_URL"),
        }
    }

    #[test]
    fn the_data_host_sits_beside_the_portal_host_and_falls_back_to_it() {
        let at = |public: &str| {
            let public = public.to_owned();
            Config::from_vars(move |k| (k == "JC_PORTAL_PUBLIC_URL").then(|| public.clone()))
                .expect("valid")
                .api_base_url()
                .to_string()
        };
        assert_eq!(
            at("https://portal.dev.example.com"),
            "https://api.dev.example.com/"
        );
        assert_eq!(
            at("https://portal.example.sk:8443/"),
            "https://api.example.sk:8443/"
        );
        assert_eq!(at("https://bb.example.sk"), "https://bb.example.sk/");
        assert_eq!(at("http://localhost:8080"), "http://localhost:8080/");
    }

    #[test]
    fn partial_oidc_configuration_is_rejected() {
        let err = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            _ => None,
        })
        .expect_err("partial oidc config must fail closed");
        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_OIDC_ISSUER"),
        }
    }

    #[test]
    fn a_portal_people_sign_in_to_does_not_start_without_its_cookie_key() {
        let err = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            _ => None,
        })
        .expect_err("no cookie key");
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "JC_PORTAL_COOKIE_KEY",
                ..
            }
        ));
        assert!(
            Config::from_vars(|_| None).is_ok(),
            "without a realm nobody signs in"
        );
    }

    #[test]
    fn complete_oidc_configuration_is_accepted_and_redacts_the_secret() {
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            _ => None,
        })
        .expect("oidc config");
        let oidc = config.oidc.as_ref().expect("oidc present");
        assert_eq!(oidc.client_id, "portal");
        assert!(matches!(&oidc.auth, ClientAuth::Secret(secret) if secret == "s3cr3t"));
        let dumped = format!("{config:?}");
        assert!(
            !dumped.contains("s3cr3t"),
            "client secret leaked into Debug: {dumped}"
        );
        assert!(dumped.contains("[redacted]"));
    }

    #[test]
    fn the_reconcilers_client_authenticates_one_way_only() {
        let err = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID" => Some("portal-reconciler".to_string()),
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET" => Some("old".to_string()),
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT" => {
                Some("portal-reconciler".to_string())
            }
            _ => None,
        })
        .expect_err("a secret beside a ServiceAccount is a fallback nobody asked for");
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT",
                ..
            }
        ));

        // A ServiceAccount without a realm has no audience to mint for.
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID" => Some("portal-reconciler".to_string()),
            "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT" => {
                Some("portal-reconciler".to_string())
            }
            _ => None,
        })
        .expect_err("no issuer");
        assert!(
            matches!(err, ConfigError::Invalid { reason, .. } if reason.contains("JC_OIDC_ISSUER"))
        );
    }

    #[test]
    fn a_federated_login_client_reads_its_token_file_and_holds_no_secret() {
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api".to_string()),
            "JC_OIDC_CLIENT_ASSERTION_FILE" => {
                Some("/var/run/secrets/jc/keycloak/token".to_string())
            }
            _ => None,
        })
        .expect("oidc config");
        assert!(matches!(
            config.oidc.expect("oidc").auth,
            ClientAuth::Assertion(path) if path == std::path::Path::new("/var/run/secrets/jc/keycloak/token")
        ));
    }

    #[test]
    fn oidc_ca_file_is_read_from_disk() {
        let path = std::env::temp_dir().join(format!("jc-portal-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"-----BEGIN CERTIFICATE-----\nnot-a-real-one\n").expect("write pem");
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_OIDC_CA_FILE" => Some(path.display().to_string()),
            _ => None,
        })
        .expect("oidc config with a ca file");
        let _ = std::fs::remove_file(&path);
        let pem = config
            .oidc
            .as_ref()
            .expect("oidc present")
            .extra_ca_pem
            .as_ref()
            .expect("ca pem loaded");
        assert!(pem.starts_with(b"-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn oidc_configuration_without_a_ca_file_carries_no_extra_root() {
        let config = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            _ => None,
        })
        .expect("oidc config");
        assert!(config.oidc.expect("oidc present").extra_ca_pem.is_none());
    }

    #[test]
    fn unreadable_oidc_ca_file_stops_start_up() {
        let err = Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.sk/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_OIDC_CA_FILE" => Some("/nonexistent/jc-portal/ca.crt".to_string()),
            _ => None,
        })
        .expect_err("a mount that is not there must fail closed, not lose the root silently");
        match err {
            ConfigError::Invalid { var, reason } => {
                assert_eq!(var, "JC_OIDC_CA_FILE");
                assert!(
                    reason.contains("/nonexistent/jc-portal/ca.crt"),
                    "the operator needs the path in the message: {reason}"
                );
            }
        }
    }

    #[test]
    fn short_cookie_key_is_rejected() {
        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_COOKIE_KEY" => Some("too-short".to_string()),
            _ => None,
        })
        .expect_err("short cookie key must fail");
        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_COOKIE_KEY"),
        }
    }

    #[test]
    fn redirect_uri_is_derived_from_the_public_base_url() {
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.sk".to_string()),
            _ => None,
        })
        .expect("config");
        assert_eq!(
            config.redirect_uri(),
            "https://portal.example.sk/api/v1/auth/callback"
        );
    }

    #[test]
    fn for_tests_provides_valid_config() {
        let cfg = Config::for_tests();
        assert_eq!(cfg.bind.port(), 0);
    }

    #[test]
    fn sync_interval_and_webhook_secret_configuration() {
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_SYNC_INTERVAL" => Some("30".to_string()),
            "JC_GITEA_WEBHOOK_SECRET" => Some("my-secret".to_string()),
            _ => None,
        })
        .expect("config");
        assert_eq!(config.sync_interval, Duration::from_secs(30));
        assert_eq!(config.gitea_webhook_secret.as_deref(), Some("my-secret"));

        let debug = format!("{config:?}");
        assert!(!debug.contains("my-secret"));
        assert!(debug.contains("[redacted]"));

        let err = Config::from_vars(|k| match k {
            "JC_PORTAL_SYNC_INTERVAL" => Some("not-a-number".to_string()),
            _ => None,
        })
        .expect_err("should reject invalid sync interval");
        match err {
            ConfigError::Invalid { var, .. } => assert_eq!(var, "JC_PORTAL_SYNC_INTERVAL"),
        }
    }

    /// T-6509: Grafana is all four variables or none, and its password never prints.
    #[test]
    fn grafana_is_all_of_its_variables_or_none() {
        let all = |name: &str| match name {
            "JC_PORTAL_GRAFANA_URL" => Some("http://grafana:3000/grafana/".to_owned()),
            "JC_PORTAL_GRAFANA_PUBLIC_URL" => Some("https://dev.example/grafana".to_owned()),
            "JC_PORTAL_GRAFANA_USER" => Some("admin".to_owned()),
            "JC_PORTAL_GRAFANA_PASSWORD" => Some("a-password".to_owned()),
            _ => None,
        };
        let config = grafana_config(&all).expect("valid").expect("configured");
        assert_eq!(config.url, "http://grafana:3000/grafana");
        assert!(!format!("{config:?}").contains("a-password"));
        assert!(grafana_config(&|_| None).expect("valid").is_none());
        let half = |name: &str| all(name).filter(|_| name != "JC_PORTAL_GRAFANA_PASSWORD");
        assert!(grafana_config(&half).is_err());
    }

    /// T-0411, AP-18: a Portal deploys an app only when it is told where. Guessing a namespace
    /// would mean writing a Deployment into somebody else's.
    #[test]
    fn build_pods_start_only_with_a_namespace_and_then_need_pinned_images() {
        let complete = |k: &str| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("https://bb.example.sk".to_string()),
            "JC_PORTAL_BUILD_NAMESPACE" => Some("dev".to_string()),
            "JC_PORTAL_BUILD_IMAGE_NODE" => Some("r.example.org/builder@sha256:aa".to_string()),
            "JC_PORTAL_BUILD_IMAGE_RUST" => Some("r.example.org/rust@sha256:bb".to_string()),
            "JC_GITEA_URL" => Some("http://gitea-http.dev.svc.cluster.local:3000".to_string()),
            _ => None,
        };
        let settings = Config::from_vars(complete)
            .expect("a complete configuration")
            .build_pods
            .expect("every part is there");
        assert_eq!(settings.cache_size, "1Gi");
        assert_eq!(
            settings.forge,
            "http://gitea-http.dev.svc.cluster.local:3000"
        );

        let without_namespace = Config::from_vars(|k| match k {
            "JC_PORTAL_BUILD_NAMESPACE" => None,
            other => complete(other),
        })
        .expect("a Portal without build pods is still a Portal");
        assert!(without_namespace.build_pods.is_none());

        for (var, value) in [
            ("JC_PORTAL_BUILD_IMAGE_RUST", None),
            (
                "JC_PORTAL_BUILD_IMAGE_NODE",
                Some("r.example.org/builder:latest"),
            ),
            ("JC_PORTAL_BUILD_NAMESPACE", Some("Dev_ns")),
            ("JC_PORTAL_BUILD_CACHE_SIZE", Some("1TB")),
            ("JC_PORTAL_BUILD_CACHE_SIZE", Some("Gi")),
            ("JC_GITEA_URL", None),
        ] {
            let refused = Config::from_vars(|k| match k == var {
                true => value.map(str::to_owned),
                false => complete(k),
            });
            assert!(refused.is_err(), "{var}={value:?} was accepted");
        }
    }

    #[test]
    fn app_settings_need_every_part_and_derive_the_one_they_can() {
        let complete = |k: &str| match k {
            "JC_PORTAL_PUBLIC_URL" => Some("https://bb.example.sk".to_string()),
            "JC_PORTAL_APPS_NAMESPACE" => Some("joinedcontext".to_string()),
            "JC_PORTAL_ORG_DOMAIN" => Some("banskabystrica.sk".to_string()),
            _ => None,
        };
        let settings = Config::from_vars(complete)
            .expect("a complete configuration")
            .app_settings
            .expect("every part is there");
        assert_eq!(settings.namespace, "joinedcontext");
        assert_eq!(settings.org_domain, "banskabystrica.sk");
        // Not a variable of its own: the host is the Portal's public URL. No realm and no
        // sidecar image any more: the login front is the edge's one `edge` client (ADR-N-019).
        assert_eq!(settings.host, "bb.example.sk");
        assert_eq!(settings.apex, "bb.example.sk");
        assert_eq!(settings.gateway_url, None);

        // AP-133, AP-134: the Apps' hosts sit under the apps origin's host, else under the
        // Portal's host without its `portal.` label; the gateway is the Portal's own.
        let apex = |extra: &'static [(&'static str, &'static str)]| {
            Config::from_vars(|k| {
                extra
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, value)| (*value).to_owned())
                    .or_else(|| complete(k))
            })
            .expect("a complete configuration")
            .app_settings
            .expect("every part is there")
        };
        let on_dev = apex(&[
            ("JC_PORTAL_PUBLIC_URL", "https://portal.dev.example.com"),
            (
                "JC_PORTAL_GATEWAY_URL",
                "http://context-gateway.dev.svc.cluster.local:8080/",
            ),
        ]);
        assert_eq!(on_dev.host, "portal.dev.example.com");
        assert_eq!(on_dev.apex, "dev.example.com");
        assert_eq!(
            on_dev.gateway_url.as_deref(),
            Some("http://context-gateway.dev.svc.cluster.local:8080")
        );
        let named = apex(&[
            ("JC_PORTAL_PUBLIC_URL", "https://portal.dev.example.com"),
            ("JC_PORTAL_APPS_URL", "https://city.example.org"),
        ]);
        assert_eq!(named.apex, "city.example.org");

        // AP-67: a pod App is handed its basemap only where the Portal proxies one.
        assert_eq!(settings.basemap_base, None);
        let mapped = apex(&[
            (
                "JC_BASEMAP_URL",
                "https://tiles.example.com/{z}/{x}/{y}.png",
            ),
            ("JC_BASEMAP_ATTRIBUTION", "OpenStreetMap"),
        ]);
        assert_eq!(
            mapped.basemap_base.as_deref(),
            Some("https://bb.example.sk/")
        );

        for missing in ["JC_PORTAL_APPS_NAMESPACE", "JC_PORTAL_ORG_DOMAIN"] {
            let config = Config::from_vars(|k| match k == missing {
                true => None,
                false => complete(k),
            })
            .expect("a Portal without apps is still a Portal");
            assert!(config.app_settings.is_none(), "{missing} was not needed");
        }
    }

    /// AP-109, AP-134 (T-3037): an App pod asks its caller's roles on the Portal's Service in the
    /// cluster; anything that does not name a `{service}.{namespace}` host with no path stops the
    /// Portal at start, since the pod's NetworkPolicy would admit some other address.
    #[test]
    fn app_settings_take_the_portal_service_only_as_an_in_cluster_address() {
        let with = |value: &'static str| {
            move |k: &str| match k {
                "JC_PORTAL_PUBLIC_URL" => Some("https://bb.example.sk".to_string()),
                "JC_PORTAL_APPS_NAMESPACE" => Some("joinedcontext".to_string()),
                "JC_PORTAL_ORG_DOMAIN" => Some("banskabystrica.sk".to_string()),
                "JC_PORTAL_SERVICE_URL" => Some(value.to_string()),
                _ => None,
            }
        };
        let settings = Config::from_vars(with("http://portal.portal.svc.cluster.local:8080/"))
            .expect("an in-cluster Service")
            .app_settings
            .expect("every part is there");
        assert_eq!(
            settings.service_url.as_deref(),
            Some("http://portal.portal.svc.cluster.local:8080")
        );
        for refused in [
            "not a url",
            "ftp://portal.portal.svc.cluster.local:8080",
            "http://portal:8080",
            "http://portal.portal.svc.cluster.local:8080/api",
        ] {
            let err = Config::from_vars(with(refused)).expect_err(refused);
            assert!(
                err.to_string().contains("JC_PORTAL_SERVICE_URL"),
                "{refused}: {err}"
            );
        }
    }

    /// AP-108: a pod-backed App's image is composed from the registry and the forge's
    /// organization, pulled with the named Secret, and reached from the configured APISIX
    /// namespace; a registry that is not a bare host, or a name that is not a Kubernetes name,
    /// stops the Portal at start rather than rendering a pod no node can pull.
    #[test]
    fn app_settings_compose_the_image_repository_and_refuse_what_no_node_resolves() {
        let with = |extra: Vec<(&'static str, &'static str)>| {
            move |k: &str| match k {
                "JC_PORTAL_PUBLIC_URL" => Some("https://bb.example.sk".to_string()),
                "JC_PORTAL_APPS_NAMESPACE" => Some("joinedcontext".to_string()),
                "JC_PORTAL_ORG_DOMAIN" => Some("banskabystrica.sk".to_string()),
                _ => extra
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, value)| (*value).to_string()),
            }
        };
        let plain = Config::from_vars(with(vec![]))
            .unwrap()
            .app_settings
            .unwrap();
        assert_eq!(plain.apisix_namespace, "apisix");
        assert_eq!((plain.image_repository, plain.pull_secret), (None, None));

        let dev = Config::from_vars(with(vec![
            ("JC_PORTAL_APPS_REGISTRY", "2.28.67.127.sslip.io"),
            ("JC_GITEA_OWNER", "joinedcontext"),
            ("JC_PORTAL_APPS_PULL_SECRET_NAME", "app-registry"),
            ("JC_PORTAL_APISIX_NAMESPACE", "dev"),
        ]))
        .unwrap()
        .app_settings
        .unwrap();
        assert_eq!(
            dev.image_of("air-quality", "sha256:ab").as_deref(),
            Some("2.28.67.127.sslip.io/joinedcontext/app-air-quality@sha256:ab")
        );
        assert_eq!(dev.pull_secret.as_deref(), Some("app-registry"));
        assert_eq!(dev.apisix_namespace, "dev");

        // PF-106: an installation whose applications have their own organization publishes and
        // pulls their images there.
        let apart = Config::from_vars(with(vec![
            ("JC_PORTAL_APPS_REGISTRY", "2.28.67.127.sslip.io"),
            ("JC_GITEA_OWNER", "joinedcontext"),
            ("JC_GITEA_APPS_OWNER", "joinedcontext-apps"),
        ]))
        .unwrap()
        .app_settings
        .unwrap();
        assert_eq!(
            apart.image_of("air-quality", "sha256:ab").as_deref(),
            Some("2.28.67.127.sslip.io/joinedcontext-apps/app-air-quality@sha256:ab")
        );

        for (var, value) in [
            ("JC_PORTAL_APPS_REGISTRY", "https://forge.example.org"),
            ("JC_PORTAL_APPS_REGISTRY", "forge.example.org/git"),
            ("JC_PORTAL_APPS_REGISTRY", "forge.example.org:"),
            ("JC_PORTAL_APPS_REGISTRY", "Forge.example.org"),
            ("JC_PORTAL_APPS_PULL_SECRET_NAME", "App_Registry"),
            ("JC_PORTAL_APISIX_NAMESPACE", "dev/x"),
        ] {
            match Config::from_vars(with(vec![
                (var, value),
                ("JC_GITEA_OWNER", "joinedcontext"),
            ])) {
                Err(ConfigError::Invalid { var: refused, .. }) => {
                    assert_eq!(refused, var, "{value}")
                }
                Ok(_) => panic!("{var}={value} was accepted"),
            }
        }
        match Config::from_vars(with(vec![("JC_PORTAL_APPS_REGISTRY", "forge.example.org")])) {
            Err(ConfigError::Invalid { reason, .. }) => assert!(reason.contains("JC_GITEA_OWNER")),
            Ok(_) => panic!("a registry without the forge's organization was accepted"),
        }
    }

    /// T-2874: dashboards are hidden unless the literal `true` shows them.
    #[test]
    fn dashboards_are_shown_only_when_asked_for_in_so_many_words() {
        assert!(!Config::from_vars(|_| None).unwrap().dashboards);
        for (value, shown) in [("1", false), ("TRUE", false), ("", false), (" true ", true)] {
            let config =
                Config::from_vars(|k| (k == "JC_PORTAL_DASHBOARDS").then(|| value.to_string()))
                    .unwrap();
            assert_eq!(config.dashboards, shown, "{value:?}");
        }
    }

    /// T-5610: the connector clients are always refused REST writes; the setting only adds.
    #[test]
    fn the_connector_clients_are_the_defaults_plus_what_the_deployment_adds() {
        assert_eq!(
            Config::from_vars(|_| None).unwrap().connector_clients,
            ["mcp-mobile", "mcp-hub", "mcp-local"]
        );
        for value in ["", " , ", "mcp-mobile"] {
            let config = Config::from_vars(|k| match k {
                "JC_PORTAL_CONNECTOR_CLIENTS" => Some(value.to_string()),
                _ => None,
            })
            .unwrap();
            assert_eq!(
                config.connector_clients,
                ["mcp-mobile", "mcp-hub", "mcp-local"],
                "{value:?}"
            );
        }
        let config = Config::from_vars(|k| match k {
            "JC_PORTAL_CONNECTOR_CLIENTS" => Some(" claude-desktop ,mcp-hub".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            config.connector_clients,
            ["mcp-mobile", "mcp-hub", "mcp-local", "claude-desktop"]
        );
    }

    /// ADR-N-019: the edge token is trusted on the literal `true` and on nothing else.
    #[test]
    fn the_edge_token_is_trusted_only_when_asked_for_in_so_many_words() {
        assert!(!Config::from_vars(|_| None).unwrap().trust_edge_token);
        for value in ["1", "yes", "TRUE", ""] {
            let config = Config::from_vars(|k| match k {
                "JC_TRUST_EDGE_TOKEN" => Some(value.to_string()),
                _ => None,
            })
            .unwrap();
            assert!(!config.trust_edge_token, "{value:?} opened the door");
        }
        let config = Config::from_vars(|k| match k {
            "JC_TRUST_EDGE_TOKEN" => Some("true".to_string()),
            _ => None,
        })
        .unwrap();
        assert!(config.trust_edge_token);
    }

    /// T-2748: the setup statements are read as said, and unset, blank or misspelled is "not said".
    #[test]
    fn the_setup_statements_say_only_what_the_deployment_said() {
        let silent = Config::from_vars(|_| None).unwrap().setup;
        assert_eq!(silent, SetupStatements::default());
        let said = Config::from_vars(|k| match k {
            "JC_SETUP_LOGIN_THEME" => Some(" joinedcontext ".to_string()),
            "JC_SETUP_SMTP" => Some("true".to_string()),
            "JC_SETUP_BACKUPS" => Some("yes".to_string()),
            _ => None,
        })
        .unwrap()
        .setup;
        assert_eq!(said.login_theme.as_deref(), Some("joinedcontext"));
        assert!(said.smtp && !said.backups);
        let blank = Config::from_vars(|k| (k == "JC_SETUP_LOGIN_THEME").then(|| "  ".to_string()))
            .unwrap()
            .setup;
        assert_eq!(blank.login_theme, None);
    }

    /// T-3359: the store's public origin is one https origin, normalized, or the Portal refuses
    /// to start saying what to write.
    #[test]
    fn the_apps_store_origin_is_one_https_origin() {
        let read = |value: &str| {
            let value = value.to_owned();
            Config::from_vars(move |k| (k == "JC_PORTAL_APPS_STORE_ORIGIN").then(|| value.clone()))
        };
        assert_eq!(
            read("https://files.dev.example.com/")
                .expect("valid")
                .apps_store_origin
                .as_deref(),
            Some("https://files.dev.example.com")
        );
        assert_eq!(
            read("https://files.example.com:9443")
                .expect("valid")
                .apps_store_origin
                .as_deref(),
            Some("https://files.example.com:9443")
        );
        assert_eq!(read("  ").expect("unset").apps_store_origin, None);
        for bad in [
            "http://files.example.com",
            "https://files.example.com/apps",
            "https://*.example.com",
            "https://u:p@files.example.com",
            "https://files.example.com/?x=1",
            "files.example.com",
        ] {
            let err = read(bad).expect_err(bad);
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "JC_PORTAL_APPS_STORE_ORIGIN",
                        ..
                    }
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn basemap_configuration_validation() {
        let config = Config::from_vars(|k| match k {
            "JC_BASEMAP_URL" => Some("https://tiles.example.com/{z}/{x}/{y}.png".to_string()),
            "JC_BASEMAP_ATTRIBUTION" => Some("OpenStreetMap".to_string()),
            _ => None,
        })
        .expect("valid basemap config");
        let bm = config.basemap.expect("basemap present");
        assert_eq!(bm.max_zoom, 19);
        assert_eq!(bm.attribution, "OpenStreetMap");
        assert!(!bm.allow_http);

        // Missing attribution when url is present
        let err = Config::from_vars(|k| match k {
            "JC_BASEMAP_URL" => Some("https://tiles.example.com/{z}/{x}/{y}.png".to_string()),
            _ => None,
        })
        .expect_err("attribution required");
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "JC_BASEMAP_ATTRIBUTION",
                ..
            }
        ));

        // http url is rejected from environment
        let err = Config::from_vars(|k| match k {
            "JC_BASEMAP_URL" => Some("http://tiles.example.com/{z}/{x}/{y}.png".to_string()),
            "JC_BASEMAP_ATTRIBUTION" => Some("OSM".to_string()),
            _ => None,
        })
        .expect_err("http rejected");
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "JC_BASEMAP_URL",
                ..
            }
        ));
    }
}
