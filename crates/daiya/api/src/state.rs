#![forbid(unsafe_code)]
//! Shared application state and request context extraction for `daiya-api`.

use crate::public::PublicRateLimiter;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use daiya_model::{
    Authorizer, CrError, DaiyaStore, InputLimits, InstallationId, InstanceId, OperationRegistry,
    OrgId, PolicyAuthorizer, RequestCtx, SERVER_ADMIN_ACTOR, ServerAdminAuthorizer, Subject,
    constant_time_eq, daiya_registry, sha256_hex,
};
use daiya_model::{CrResult, DatasourceRecord};
use daiya_query::{
    Datasource, DenyTokenExchanger, DriverDeps, DsError, MapSecretResolver, OutboundPolicy,
    QueryCache, SecretResolver, TokenExchanger, WasmPluginHost, Zeroizing, build_driver,
};
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Live datasource drivers keyed by `(org_id, datasource_uid)`.
pub type DriverMap = BTreeMap<(OrgId, String), Arc<dyn Datasource>>;

/// Drivers built from stored records, with the record revision (`version`, `updatedAt`) they
/// were built from: a changed record builds a new driver.
pub type BuiltDriverMap = BTreeMap<(OrgId, String), (String, Arc<dyn Datasource>)>;

/// Host-supplied resolver of a verified [`Subject`] (the Portal or jc binary that mounts daiya
/// in-process and has already verified the session or bearer token). `None` means "not mine".
pub type SubjectResolver = Arc<dyn Fn(&HeaderMap) -> Option<Subject> + Send + Sync>;

/// Host-supplied reader of the dashboard page (`/d/{uid}`) for a browser that carries no
/// credential: given the request's headers and the organization's number and name, the person
/// the host's own session names, when that person may read the organization's dashboards
/// (PORTAL-CALLS.md G-5). daiya keeps no sessions; `None` leaves the browser anonymous.
pub type PageReader = Arc<dyn Fn(&HeaderMap, i64, &str) -> Option<String> + Send + Sync>;

/// One API credential: the SHA-256 of the token (never the token) and the subject it stands for.
#[derive(Debug, Clone)]
pub struct TokenBinding {
    pub token_sha256: String,
    pub subject: Subject,
}

impl TokenBinding {
    /// Binds the raw `token` (hashed here, the raw value is not kept) to `subject`.
    #[must_use]
    pub fn new(token: &str, subject: Subject) -> Self {
        Self {
            token_sha256: sha256_hex(token.as_bytes()),
            subject,
        }
    }
}

/// Shared state of a `daiya` instance (R11: passed in as a struct, no globals or env reads).
#[derive(Clone)]
pub struct DaiyaState {
    pub installation_id: InstallationId,
    pub default_org_id: OrgId,
    pub default_instance_id: InstanceId,
    pub store: Arc<dyn DaiyaStore>,
    pub authorizer: Arc<dyn Authorizer>,
    /// Live datasource drivers keyed by `(org_id, datasource_uid)`.
    pub datasources: Arc<RwLock<DriverMap>>,
    /// Mapping between Grafana numeric `orgId` / org name and `(numeric_id, OrgId)` for `provision.sh` and `src/grafana.rs`.
    pub org_registry: Arc<RwLock<OrgDirectory>>,
    pub query_cache: Arc<QueryCache>,
    pub wasm_host: Arc<WasmPluginHost>,
    pub secret_resolver: Arc<dyn SecretResolver>,
    pub token_exchanger: Arc<dyn TokenExchanger>,
    pub limits: InputLimits,
    pub public_rate_limiter: Arc<PublicRateLimiter>,
    pub registry: Arc<OperationRegistry>,
    /// API credentials (hashed) accepted as `Authorization: Bearer <token>` or `Basic <base64>`.
    pub tokens: Arc<Vec<TokenBinding>>,
    /// Verified subject from the host that mounts daiya (checked before `tokens`).
    pub subject_resolver: Option<SubjectResolver>,
    /// Reads the dashboard page for a signed-in browser ([`DaiyaState::with_page_reader`]).
    pub page_reader: Option<PageReader>,
    /// Operator outbound settings for drivers built from stored datasource records.
    pub outbound_policy: OutboundPolicy,
    /// Drivers built from stored datasource records (see [`DaiyaState::driver_for`]).
    pub built_drivers: Arc<RwLock<BuiltDriverMap>>,
    /// Checks that recognise the server administrator ([`DaiyaState::with_server_admin`],
    /// [`DaiyaState::with_server_admin_check`]); none by default.
    pub server_admin: Vec<ServerAdminCheck>,
    /// The host's own gateway Endpoints, reachable by every NGSI-LD and OData datasource that
    /// names one (see [`daiya_query::OwnReach`]); empty by default.
    pub own_reach: Vec<daiya_query::OwnReach>,
}

/// A host-supplied check that a request comes from the server administrator, e.g. a kaisatsu
/// token verified in-process with daiya's audience (PORTAL-CALLS.md §5).
pub type ServerAdminCheck = Arc<dyn Fn(&HeaderMap) -> bool + Send + Sync>;

/// The shortest server administrator password accepted.
pub const MIN_SERVER_ADMIN_PASSWORD: usize = 16;

/// Directory mapping Grafana organization names and numeric IDs to UUIDv7 [`OrgId`]s.
#[derive(Debug, Default)]
pub struct OrgDirectory {
    pub by_name: BTreeMap<String, (i64, OrgId)>,
    pub by_num: BTreeMap<i64, (String, OrgId)>,
    pub next_num: i64,
}

impl OrgDirectory {
    #[must_use]
    pub fn with_main_org(main_org_id: OrgId) -> Self {
        let mut dir = Self {
            by_name: BTreeMap::new(),
            by_num: BTreeMap::new(),
            next_num: 2,
        };
        dir.by_name.insert("Main Org.".to_owned(), (1, main_org_id));
        dir.by_num.insert(1, ("Main Org.".to_owned(), main_org_id));
        dir
    }

    pub fn register_org(&mut self, name: &str, org_id: OrgId) -> (i64, OrgId, bool) {
        if let Some(&(num, existing_id)) = self.by_name.get(name) {
            return (num, existing_id, false);
        }
        let num = self.next_num;
        self.next_num += 1;
        self.by_name.insert(name.to_owned(), (num, org_id));
        self.by_num.insert(num, (name.to_owned(), org_id));
        (num, org_id, true)
    }
}

impl DaiyaState {
    #[must_use]
    pub fn new(
        installation_id: InstallationId,
        default_org_id: OrgId,
        default_instance_id: InstanceId,
        store: Arc<dyn DaiyaStore>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            installation_id,
            default_org_id,
            default_instance_id,
            store,
            authorizer,
            datasources: Arc::new(RwLock::new(BTreeMap::new())),
            org_registry: Arc::new(RwLock::new(OrgDirectory::with_main_org(default_org_id))),
            query_cache: Arc::new(QueryCache::default()),
            wasm_host: Arc::new(WasmPluginHost::default()),
            secret_resolver: Arc::new(MapSecretResolver::new()),
            token_exchanger: Arc::new(DenyTokenExchanger),
            limits: InputLimits::default(),
            public_rate_limiter: Arc::new(PublicRateLimiter::new(120, 60)),
            registry: Arc::new(daiya_registry()),
            tokens: Arc::new(Vec::new()),
            subject_resolver: None,
            page_reader: None,
            outbound_policy: OutboundPolicy::default(),
            built_drivers: Arc::new(RwLock::new(BTreeMap::new())),
            server_admin: Vec::new(),
            own_reach: Vec::new(),
        }
    }

    /// Reads NGSI-LD and OData Endpoints with the person's token the Portal hands over in
    /// [`PERSON_TOKEN_HEADER`] (see [`PersonTokenExchanger`]).
    #[must_use]
    pub fn with_person_tokens(mut self) -> Self {
        self.token_exchanger = Arc::new(PersonTokenExchanger);
        self
    }

    /// Grafana's local administrator: `Authorization: Basic base64(user:password)`, as
    /// `provision.sh` and the Portal (`JC_PORTAL_GRAFANA_USER`/`_PASSWORD`) send it, acts as
    /// [`SERVER_ADMIN_ACTOR`] in every organization (see [`ServerAdminAuthorizer`]). Only the
    /// digest is kept. Refuses an empty user, a user holding `:` and a password shorter than
    /// [`MIN_SERVER_ADMIN_PASSWORD`]; the error never repeats the password.
    pub fn with_server_admin(self, user: &str, password: &str) -> CrResult<Self> {
        use base64::Engine as _;
        if user.trim().is_empty() || user.contains(':') {
            return Err(daiya_model::CrError::BadRequest(
                "serverAdmin.user: a name without ':'".into(),
            ));
        }
        if password.chars().count() < MIN_SERVER_ADMIN_PASSWORD {
            return Err(daiya_model::CrError::BadRequest(format!(
                "serverAdmin.password: at least {MIN_SERVER_ADMIN_PASSWORD} characters"
            )));
        }
        let credential =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        let expected = sha256_hex(credential.as_bytes());
        Ok(self.with_server_admin_check(move |headers| basic_matches(headers, &expected)))
    }

    /// Recognises the server administrator by `check` (beside any check already set): a host
    /// that verifies a kaisatsu token with daiya's audience passes it here.
    #[must_use]
    pub fn with_server_admin_check<F>(mut self, check: F) -> Self
    where
        F: Fn(&HeaderMap) -> bool + Send + Sync + 'static,
    {
        if self.server_admin.is_empty() {
            self.authorizer = Arc::new(ServerAdminAuthorizer {
                inner: self.authorizer.clone(),
            });
        }
        self.server_admin.push(Arc::new(check));
        self
    }

    /// The server administrator, when one of its checks recognises the request: a member of
    /// every organization the directory holds, the main one first.
    fn server_admin_for(&self, headers: &HeaderMap) -> Option<Subject> {
        if !self.server_admin.iter().any(|check| check(headers)) {
            return None;
        }
        let organizations = self
            .org_registry
            .read()
            .by_num
            .values()
            .map(|(_, org)| *org)
            .collect();
        Some(Subject::authenticated(
            SERVER_ADMIN_ACTOR,
            organizations,
            Vec::new(),
        ))
    }

    /// Accepts these API credentials (hashed bindings).
    #[must_use]
    pub fn with_tokens(mut self, tokens: Vec<TokenBinding>) -> Self {
        self.tokens = Arc::new(tokens);
        self
    }

    /// Takes the verified subject from the mounting host before looking at API credentials.
    #[must_use]
    pub fn with_subject_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&HeaderMap) -> Option<Subject> + Send + Sync + 'static,
    {
        self.subject_resolver = Some(Arc::new(resolver));
        self
    }

    /// Lets the person `reader` names read the dashboard page of an organization, and only that
    /// page: its dashboard and the saved panels on it, as the Portal draws them for a member.
    #[must_use]
    pub fn with_page_reader<F>(mut self, reader: F) -> Self
    where
        F: Fn(&HeaderMap, i64, &str) -> Option<String> + Send + Sync + 'static,
    {
        self.page_reader = Some(Arc::new(reader));
        self
    }

    /// The subject bound to the credential in `Authorization`, compared on SHA-256 digests in
    /// constant time; `None` for a missing or unknown credential.
    #[must_use]
    pub fn subject_for_credential(&self, headers: &HeaderMap) -> Option<Subject> {
        let raw = headers
            .get(axum::http::header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .trim();
        let (scheme, credential) = raw.split_once(' ')?;
        let credential = credential.trim();
        if credential.is_empty() {
            return None;
        }
        let is_bearer = scheme.eq_ignore_ascii_case("bearer");
        if !is_bearer && !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let digest = sha256_hex(credential.as_bytes());
        let mut found: Option<&TokenBinding> = None;
        for binding in self.tokens.iter() {
            if constant_time_eq(&binding.token_sha256, &digest) && found.is_none() {
                found = Some(binding);
            }
        }
        let mut subject = found?.subject.clone();
        if is_bearer {
            subject.bearer_token = Some(credential.to_owned());
        }
        Some(subject)
    }

    /// Registers a runtime [`Datasource`] implementation for `(org_id, uid)`.
    pub fn register_datasource_driver(
        &self,
        org_id: OrgId,
        uid: impl Into<String>,
        driver: Arc<dyn Datasource>,
    ) {
        self.datasources
            .write()
            .insert((org_id, uid.into()), driver);
    }

    /// Looks up a registered [`Datasource`] driver for `(org_id, uid)`.
    #[must_use]
    pub fn get_datasource_driver(&self, org_id: OrgId, uid: &str) -> Option<Arc<dyn Datasource>> {
        self.datasources
            .read()
            .get(&(org_id, uid.to_owned()))
            .cloned()
    }

    /// The driver for `(org_id, uid)`: a driver registered by the host first, otherwise one
    /// built from the stored datasource record (cached until the record changes). `None` when
    /// the organization has no such datasource.
    pub async fn driver_for(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> CrResult<Option<Arc<dyn Datasource>>> {
        if let Some(driver) = self.get_datasource_driver(org_id, uid) {
            return Ok(Some(driver));
        }
        let Some(record) = self.store.get_datasource(org_id, uid).await? else {
            return Ok(None);
        };
        self.driver_for_record(&record).await.map(Some)
    }

    /// The driver built from `record` (see [`daiya_query::build_driver`]), cached by record
    /// revision so a Postgres pool or a resolved address is reused until the record changes.
    pub async fn driver_for_record(
        &self,
        record: &DatasourceRecord,
    ) -> CrResult<Arc<dyn Datasource>> {
        if let Some(driver) = self.get_datasource_driver(record.org_id, &record.uid) {
            return Ok(driver);
        }
        let key = (record.org_id, record.uid.clone());
        let revision = format!("{}@{}", record.version, record.updated_at.to_rfc3339());
        if let Some((built_rev, driver)) = self.built_drivers.read().get(&key)
            && *built_rev == revision
        {
            return Ok(driver.clone());
        }
        let mut policy = self.outbound_policy.clone();
        policy.own_reach.clone_from(&self.own_reach);
        let deps = DriverDeps {
            secrets: self.secret_resolver.clone(),
            token_exchanger: self.token_exchanger.clone(),
            policy,
        };
        let driver = build_driver(record, &deps).await?;
        self.built_drivers
            .write()
            .insert(key, (revision, driver.clone()));
        Ok(driver)
    }

    /// Convenience constructor with a permissive policy for standalone tests.
    #[must_use]
    pub fn for_standalone(store: Arc<dyn DaiyaStore>, default_org_id: OrgId) -> Self {
        let authz = Arc::new(PolicyAuthorizer::allow_org_members(default_org_id, "admin"));
        Self::new(
            InstallationId::from_u128(1),
            default_org_id,
            InstanceId::from_u128(1),
            store,
            authz,
        )
    }
}

/// Whether `Authorization` is `Basic` with the credential whose SHA-256 is `expected`,
/// compared in constant time.
fn basic_matches(headers: &HeaderMap, expected: &str) -> bool {
    let Some((scheme, credential)) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.trim().split_once(' '))
    else {
        return false;
    };
    scheme.eq_ignore_ascii_case("basic")
        && constant_time_eq(expected, &sha256_hex(credential.trim().as_bytes()))
}

/// The header in which the Portal hands over a person's token, already exchanged for the one
/// Endpoint the panel reads, beside its server-administrator credential. Accepted only with that
/// credential; on any other request it is ignored.
pub const PERSON_TOKEN_HEADER: &str = "x-daiya-person-token";

/// The server administrator reading for the person whose token is in [`PERSON_TOKEN_HEADER`]:
/// the token becomes the subject's bearer for the datasource call, and its `sub` (or, without
/// one, a digest of the token) the person the cache keys on.
fn on_behalf_of(mut admin: Subject, headers: &HeaderMap) -> Subject {
    let Some(token) = headers
        .get(PERSON_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return admin;
    };
    let person =
        token_sub(token).unwrap_or_else(|| format!("token:{}", sha256_hex(token.as_bytes())));
    admin.on_behalf_of = Some(person);
    admin.bearer_token = Some(token.to_owned());
    admin
}

/// The claims of a JWT, read without verifying it: they only key the cache and pick the
/// Endpoint the token goes to; the gateway verifies the token itself.
fn claims_of(token: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The `sub` claim of a JWT (see [`claims_of`]).
#[must_use]
pub fn token_sub(token: &str) -> Option<String> {
    claims_of(token)?["sub"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The exchanger of a host whose Portal exchanges the person's token itself, for one Endpoint
/// (scope `endpoint:{slug}`), and hands it over in [`PERSON_TOKEN_HEADER`]: the token goes on
/// unchanged to the datasource of that Endpoint only. A token without that scope, or none, is
/// refused, so a person's token never reaches another Endpoint.
#[derive(Debug, Clone, Default)]
pub struct PersonTokenExchanger;

impl TokenExchanger for PersonTokenExchanger {
    fn exchange_for_audience(
        &self,
        caller_token: &str,
        audience: &str,
    ) -> Result<Zeroizing<String>, DsError> {
        let wanted = format!("endpoint:{audience}");
        let scoped = !audience.is_empty()
            && claims_of(caller_token).is_some_and(|claims| {
                claims["scope"]
                    .as_str()
                    .is_some_and(|scope| scope.split_whitespace().any(|s| s == wanted))
            });
        if scoped {
            Ok(Zeroizing::new(caller_token.to_owned()))
        } else {
            Err(DsError::TokenExchange {
                audience: audience.to_owned(),
                reason: "no token of the person for this Endpoint".into(),
            })
        }
    }
}

/// Whether `Authorization` carries a credential that names nobody: a bearer the host's
/// [`SubjectResolver`] does not verify (a bad signature, another issuer or audience, an expired
/// token) and no API credential matches, or a Basic pair that is not the server administrator's.
/// Grafana answers such a request 401; it must never fall back to anonymous.
#[must_use]
pub fn credential_refused(state: &DaiyaState, headers: &HeaderMap) -> bool {
    let present = headers
        .get(axum::http::header::AUTHORIZATION)
        .is_some_and(|value| !value.as_bytes().iter().all(u8::is_ascii_whitespace));
    present
        && state.server_admin_for(headers).is_none()
        && state
            .subject_resolver
            .as_ref()
            .and_then(|resolve| resolve(headers))
            .or_else(|| state.subject_for_credential(headers))
            .is_none()
}

/// Answers 401 to a request whose `Authorization` names nobody ([`credential_refused`]), as
/// Grafana does, before any handler reads the caller as anonymous.
pub async fn refuse_unknown_credential(
    axum::extract::State(state): axum::extract::State<DaiyaState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if credential_refused(&state, request.headers()) {
        // The answer names no reason: which check failed would help a forger.
        return error_response(&CrError::Unauthorized("invalid credential".into()));
    }
    next.run(request).await
}

/// Builds the [`RequestCtx`] of a request. The subject comes only from verified sources: the
/// mounting host's [`SubjectResolver`], then an API credential in `Authorization` (hashed,
/// constant-time). Anything else is anonymous: no organization, no role. Client headers never
/// grant identity or membership; `X-Daiya-Org-Id` / `X-Grafana-Org-Id` only choose which
/// of the subject's organizations is active (authorization still checks membership), and
/// `X-Daiya-Readonly: true` can only narrow the caller.
#[must_use]
pub fn resolve_request_ctx(state: &DaiyaState, headers: &HeaderMap) -> RequestCtx {
    let mut subject = state
        .server_admin_for(headers)
        .map(|admin| on_behalf_of(admin, headers))
        .unwrap_or_else(|| {
            let subject = state
                .subject_resolver
                .as_ref()
                .and_then(|resolve| resolve(headers))
                .or_else(|| state.subject_for_credential(headers))
                .unwrap_or_else(Subject::anonymous);
            // Only the server-admin credential makes a server administrator.
            if subject.actor_id.as_deref() == Some(SERVER_ADMIN_ACTOR) {
                Subject::anonymous()
            } else {
                subject
            }
        });

    let requested_org = headers
        .get("x-daiya-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<OrgId>().ok())
        .or_else(|| {
            let gf_org = headers.get("x-grafana-org-id")?.to_str().ok()?.trim();
            if let Ok(uuid_org) = gf_org.parse::<OrgId>() {
                return Some(uuid_org);
            }
            let num: i64 = gf_org.parse().ok()?;
            state
                .org_registry
                .read()
                .by_num
                .get(&num)
                .map(|(_, oid)| *oid)
        });
    let active_org = requested_org
        .or_else(|| subject.organizations.first().copied())
        .unwrap_or(state.default_org_id);

    if headers
        .get("x-daiya-readonly")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s == "true" || s == "1")
    {
        subject.read_only = true;
    }

    RequestCtx::new(
        state.installation_id,
        active_org,
        state.default_instance_id,
        subject,
    )
}

/// Converts a [`CrError`] into an OData 4.01 / RFC 9457 JSON HTTP response with `Cache-Control: private, no-store`.
#[must_use]
pub fn error_response(err: &CrError) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = err.to_odata_error();
    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (axum::http::header::CACHE_CONTROL, "private, no-store"),
        ],
        axum::Json(body),
    )
        .into_response()
}
