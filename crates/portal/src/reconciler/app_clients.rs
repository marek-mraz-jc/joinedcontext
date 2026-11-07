//! The Keycloak client of every published App (ADR-N-030, AP-111, T-2677).
//!
//! Each published App logs people in with a confidential client of its own, `app-{name}`: the
//! standard flow with PKCE, no direct grants, no service account, redirect and post-logout URIs
//! and the web origin of its own host `{name}.apps.{domain}` only (AP-133). This wave is the only writer of such a client: it carries the
//! attribute `managed-by: joinedcontext` and the App it belongs to, a change made in the console
//! is overwritten and reported, and a managed client whose App is no longer published is
//! deleted. A client of that id without the attribute belongs to whoever made it and is never
//! read for drift, written or removed; the App is reported as blocked instead (AP-114).
//!
//! The App's roles live on its client (AP-113): every `spec.roles[]` entry is a client role, and
//! every `spec.access[]` subject a role mapping this wave writes, a group mapping for a `group`
//! and a user mapping for a `user`. It is the only writer of both, so a role or a mapping made in
//! the console is removed and reported. Audience mappers put `app-{name}` and the slug of every
//! Endpoint the App reads into the client's tokens, so the gateway admits them there; every other
//! protocol mapper on the client is removed and named on the App (T-2857).
//!
//! Keycloak generates each secret. The wave reads it back for the edge file (AP-112) and holds
//! it in [`ClientSecret`], whose `Debug` never shows the value; it is never logged, stored in a
//! manifest or written to a ConfigMap.

use crate::config::ClientAuth;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use jc_core::kinds::{AppLifecycle, AppSpec};
use serde::Deserialize;
use serde_json::{json, Value};

use super::groups::{MANAGED_BY, MANAGED_VALUE};
use crate::resource::ResourceEnvelope;
use crate::store::Mirror;

/// The client attribute naming the App a managed client belongs to, `{project}/{name}`.
pub const APP_ATTRIBUTE: &str = "joinedcontext.app";

const TIMEOUT: Duration = Duration::from_secs(20);

/// The id of an App's client: one name for the organization, as the App name is (AP-14a).
pub fn client_id(app: &str) -> String {
    format!("app-{app}")
}

/// A client secret as Keycloak generated it. Only [`ClientSecret::expose`] reads the value.
#[derive(Clone, PartialEq, Eq)]
pub struct ClientSecret(String);

impl ClientSecret {
    /// The value, for the one place that writes it into the edge Secret.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for ClientSecret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for ClientSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientSecret(redacted)")
    }
}

/// What one run did with one App's client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOutcome {
    /// The App name, or `*` for a failure before any App was looked at.
    pub app: String,
    /// What was different from the manifest and has been written back; empty when it matched.
    pub drift: Vec<String>,
    /// A subject the realm has no group or user for yet: its mapping is written once it does
    /// (a group the group wave creates, a person at their first login), so never a failure.
    pub warnings: Vec<String>,
    /// Mappers the App does not declare that were on its client and have been removed, as
    /// [`mapper_label`]s (T-2857); the App's status names them.
    pub removed_mappers: Vec<String>,
    pub error: Option<String>,
}

impl ClientOutcome {
    pub(crate) fn of(app: &str) -> Self {
        Self {
            app: app.to_owned(),
            drift: Vec::new(),
            warnings: Vec::new(),
            removed_mappers: Vec::new(),
            error: None,
        }
    }
}

/// What a run leaves behind: one outcome per App touched, and the secret of every App whose
/// client is in place, keyed by App name.
#[derive(Debug, Default)]
pub struct ClientRun {
    pub outcomes: Vec<ClientOutcome>,
    pub secrets: BTreeMap<String, ClientSecret>,
}

/// One published App, as this wave needs it.
#[derive(Debug, Clone)]
struct PublishedApp {
    project: String,
    name: String,
    /// What the login page calls the App (PF-90): [`title_of`].
    title: String,
    /// `None` when the manifest does not parse: the client is still kept, its roles are not.
    spec: Option<AppSpec>,
    /// The names of the Organization's verified domain the edge routes for it (AP-172).
    hostnames: Vec<String>,
}

/// The published Apps of the mirror, sorted by name so a run is deterministic.
fn published(mirror: &Mirror, apex: &str) -> Vec<PublishedApp> {
    let mut apps: Vec<PublishedApp> = mirror
        .matching(|envelope| {
            envelope.kind == "App"
                && envelope.spec.get("lifecycle").and_then(Value::as_str)
                    == Some(AppLifecycle::Published.as_str())
        })
        .into_iter()
        .map(|envelope| {
            let project = envelope.metadata.namespace.clone().unwrap_or_default();
            let hostnames = crate::apps::hostnames::routed(
                mirror,
                &project,
                &envelope.metadata.name,
                &envelope.spec,
                apex,
            );
            PublishedApp {
                project,
                title: title_of(&envelope),
                name: envelope.metadata.name,
                spec: serde_json::from_value(envelope.spec).ok(),
                hostnames,
            }
        })
        .collect();
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps.dedup_by(|a, b| a.name == b.name);
    apps
}

/// The App's title as a person reads it on the login page, "Sign in to {title} · {organization}"
/// (PF-90): the English title of the legacy map, the plain one, or the App's name when it has
/// none. The realm's default language is English, and a client has one name.
pub fn title_of(app: &ResourceEnvelope) -> String {
    app.metadata
        .title
        .as_ref()
        .map(|title| title.resolve(&["en".to_owned()], "").trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| app.metadata.name.clone())
}

/// The client an App should have, as Keycloak's representation (AP-111). `host` is the apex
/// the apps' hosts sit under, `city.example.com`; `title` is [`title_of`] the App; `hostnames`
/// the names of the Organization's domain the edge routes for it (AP-172).
pub fn desired(project: &str, app: &str, title: &str, host: &str, hostnames: &[String]) -> Value {
    let origins: Vec<String> = std::iter::once(crate::reconciler::edge_file::app_host(app, host))
        .chain(hostnames.iter().cloned())
        .map(|host| format!("https://{host}"))
        .collect();
    let redirects: Vec<String> = origins.iter().map(|origin| format!("{origin}/*")).collect();
    json!({
        "clientId": client_id(app),
        "name": title,
        "protocol": "openid-connect",
        "enabled": true,
        "publicClient": false,
        "bearerOnly": false,
        "standardFlowEnabled": true,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": false,
        "frontchannelLogout": true,
        "redirectUris": redirects,
        "webOrigins": origins,
        "attributes": {
            "pkce.code.challenge.method": "S256",
            // Keycloak reads several post-logout URIs from one attribute, `##` between them.
            "post.logout.redirect.uris": redirects.join("##"),
            // The edge verifies RS256, as it does for the shared `edge` client (AP-27).
            "access.token.signed.response.alg": "RS256",
            "id.token.signed.response.alg": "RS256",
            // A role taken away or a session ended reaches every door of the App within a
            // minute: the edge renews the token on expiry, and Keycloak refuses a renewal of an
            // ended session and issues the roles held now (T-4914). The realm's own lifespan is
            // an hour on dev.
            "access.token.lifespan": "60",
            MANAGED_BY: MANAGED_VALUE,
            APP_ATTRIBUTE: format!("{project}/{app}"),
        },
    })
}

/// Who holds an App role: a Keycloak group by name, or a user by e-mail or username.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Holder {
    Group(String),
    User(String),
}

impl std::fmt::Display for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Holder::Group(name) => write!(f, "group {name}"),
            Holder::User(user) => write!(f, "user {user}"),
        }
    }
}

/// A role name as one admin path segment. The name comes from the App or from the realm, and
/// `..`, `/`, `?` or `#` in it would reach another admin route (a DELETE of another client),
/// so it is encoded, and a name that is only dots, which a URL resolves even encoded, refused.
fn role_segment(name: &str) -> Result<String, String> {
    if name.is_empty() || name.chars().all(|c| c == '.') {
        return Err(format!("the role name {name:?} cannot name a realm role"));
    }
    Ok(super::groups::urlencoding(name))
}

/// An admin path as an error says it: without its query, which can carry a person's e-mail
/// (`/users?email=…`), so a refusal in a log line or an App's status names nobody.
fn shown(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _)| path)
}

/// Each role of the App with the subjects `spec.access` gives it (AP-113). A role nobody holds is
/// still a role; an access entry naming a role the App does not declare grants nothing (jc-core
/// refuses such a manifest, and this does not trust that alone).
pub fn holders(spec: &AppSpec) -> BTreeMap<String, BTreeSet<Holder>> {
    let mut roles: BTreeMap<String, BTreeSet<Holder>> = spec
        .roles
        .iter()
        .map(|role| (role.name.clone(), BTreeSet::new()))
        .collect();
    for access in &spec.access {
        let Some(held) = roles.get_mut(&access.role) else {
            continue;
        };
        for subject in &access.subjects {
            if let Some(group) = &subject.group {
                held.insert(Holder::Group(group.clone()));
            }
            if let Some(user) = &subject.user {
                held.insert(Holder::User(user.to_ascii_lowercase()));
            }
        }
    }
    roles
}

/// The prefix of the audience mappers this wave owns on an App's client.
const AUDIENCE_MAPPER: &str = "audience-";

/// The audience mapper that puts `audience` into the client's access tokens (AP-113).
pub fn audience_mapper(audience: &str) -> Value {
    json!({
        "name": format!("{AUDIENCE_MAPPER}{audience}"),
        "protocol": "openid-connect",
        "protocolMapper": "oidc-audience-mapper",
        "config": {
            "included.custom.audience": audience,
            "access.token.claim": "true",
            "id.token.claim": "false",
            "introspection.token.claim": "true",
        },
    })
}

/// The `groups` mapper every platform client carries (deployment components/keycloak, "Identity,
/// never permission"): the person's group paths in the access, ID and userinfo tokens. Full paths
/// (ADR-N-067 2.2): every reader trims the one leading `/`, so a root group reads as its name and
/// an Organization's subgroup as `helsinki/members`, which no project, Group or grant name can
/// equal; a bare `members` would match every Organization's.
/// An App's client carries the same one, so the gateway can check a person's projects on a
/// project-bound Endpoint the App reads (AP-113, T-3033). It grants nothing by itself.
pub fn groups_mapper() -> Value {
    json!({
        "name": "groups",
        "protocol": "openid-connect",
        "protocolMapper": "oidc-group-membership-mapper",
        "consentRequired": false,
        "config": {
            "claim.name": "groups",
            "full.path": "true",
            "access.token.claim": "true",
            "id.token.claim": "true",
            "userinfo.token.claim": "true",
        },
    })
}

/// The `jc_org` and `jc_groups` mappers every platform client carries (deployment
/// components/keycloak, ADR-N-067 2.2): the person's Organizations and their groups in them, read
/// from the attributes of each `/{org}/{group}` group. The realm's user profile holds no unmanaged
/// attribute, so a person's own attribute of that name never reaches the token.
pub fn organization_mappers() -> [Value; 2] {
    ["jc_org", "jc_groups"].map(|claim| {
        json!({
            "name": claim,
            "protocol": "openid-connect",
            "protocolMapper": "oidc-usermodel-attribute-mapper",
            "consentRequired": false,
            "config": {
                "user.attribute": claim,
                "claim.name": claim,
                "jsonType.label": "String",
                "multivalued": "true",
                "aggregate.attrs": "true",
                "access.token.claim": "true",
                "id.token.claim": "true",
                "userinfo.token.claim": "true",
            },
        })
    })
}

/// The mappers an App's client carries: the audience of `app-{name}` and of every Endpoint it
/// reads, the `groups` mapper (AP-113) and the Organization mappers.
pub fn app_mappers(audiences: &[String]) -> Vec<Value> {
    audiences
        .iter()
        .map(|audience| audience_mapper(audience))
        .chain(std::iter::once(groups_mapper()))
        .chain(organization_mappers())
        .collect()
}

/// Whether a mapper held on a client is one of the `wanted` ones, by type and name (T-2857).
/// Everything else is removed, whatever its type or name.
fn allowed_mapper(mapper: &Value, wanted: &[Value]) -> bool {
    wanted.iter().any(|want| {
        mapper["protocolMapper"] == want["protocolMapper"] && mapper["name"] == want["name"]
    })
}

/// A mapper as a report names it: its type and name, never its config (a claim value can be
/// anything). Both come from the realm, so they are cut short and stripped of control
/// characters before they reach a log line or a condition.
pub fn mapper_label(mapper: &Value) -> String {
    let clean = |value: &Value| -> String {
        value
            .as_str()
            .unwrap_or("unnamed")
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect()
    };
    format!(
        "{} mapper {}",
        clean(&mapper["protocolMapper"]),
        clean(&mapper["name"])
    )
}

/// The fields of `want` the realm holds otherwise, as sentences for the drift report. Only the
/// fields [`desired`] sets are compared: Keycloak fills in many more, which are its own.
pub fn drift(want: &Value, held: &Value) -> Vec<String> {
    let mut found = Vec::new();
    let Some(want) = want.as_object() else {
        return found;
    };
    for (field, value) in want {
        if field == "attributes" {
            let held_attributes = held.get("attributes");
            for (key, wanted) in value.as_object().into_iter().flatten() {
                let current = held_attributes.and_then(|a| a.get(key));
                if current != Some(wanted) {
                    found.push(format!(
                        "attribute {key} was {}, the App says {wanted}",
                        current.map_or("unset".to_owned(), Value::to_string)
                    ));
                }
            }
            continue;
        }
        let current = held.get(field);
        if !same(current, value) {
            found.push(format!(
                "{field} was {}, the App says {value}",
                current.map_or("unset".to_owned(), Value::to_string)
            ));
        }
    }
    found
}

/// Keycloak keeps a list of strings (`redirectUris`, `webOrigins`, the client scopes) as a set
/// and answers it in its own order, so such a list is compared as a set; any other value as is.
fn same(held: Option<&Value>, want: &Value) -> bool {
    fn strings(value: &Value) -> Option<BTreeSet<&str>> {
        value.as_array()?.iter().map(Value::as_str).collect()
    }
    match (held.and_then(strings), strings(want)) {
        (Some(held), Some(want)) => held == want,
        _ => held == Some(want),
    }
}

pub(crate) fn managed(client: &Value) -> bool {
    client
        .get("attributes")
        .and_then(|a| a.get(MANAGED_BY))
        .and_then(Value::as_str)
        == Some(MANAGED_VALUE)
}

#[derive(Deserialize)]
struct KcToken {
    access_token: String,
}

/// The admin token stays out of every log line (T-4321).
impl std::fmt::Debug for KcToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KcToken")
            .field("access_token", &"[redacted]")
            .finish()
    }
}

/// A role, a group or a client scope, as the admin API lists one.
#[derive(Debug, Deserialize)]
pub(crate) struct KcNamed {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Debug, Deserialize)]
struct KcUserRef {
    id: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    email: Option<String>,
}

impl KcUserRef {
    /// Whether the App's subject names this user: a Keycloak id only by the id (ADR-N-061, a
    /// username spelling an id names nobody), a manifest written before by username or e-mail.
    fn is(&self, user: &str) -> bool {
        if crate::permissions::is_person_id(user) {
            return self.id == user;
        }
        self.username.eq_ignore_ascii_case(user)
            || self
                .email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(user))
    }
}

/// The realm's groups and users one run has looked up, so each is asked for once per run.
#[derive(Default)]
struct RealmIndex {
    groups: Option<BTreeMap<String, String>>,
    users: BTreeMap<String, Option<String>>,
}

/// The realm's admin API as the Portal's own client reaches it: a token by `client_credentials`
/// and calls under `/admin/realms/{realm}`. Shared by every wave that writes clients.
pub struct Admin {
    http: reqwest::Client,
    /// `https://idm.host/realms/{realm}`: where the token comes from.
    issuer: String,
    /// `https://idm.host/admin/realms/{realm}`: where the clients are.
    admin: String,
    client_id: String,
    auth: ClientAuth,
}

impl Admin {
    /// `None` when the issuer is not a realm URL, because then there is nothing to manage.
    pub fn new(issuer: &str, client_id: String, auth: ClientAuth) -> Option<Self> {
        let trimmed = issuer.trim_end_matches('/');
        let (root, realm) = trimmed.rsplit_once("/realms/")?;
        Some(Self {
            http: crate::auth::client_auth::realm_http(TIMEOUT)?,
            issuer: trimmed.to_owned(),
            admin: format!("{root}/admin/realms/{realm}"),
            client_id,
            auth,
        })
    }

    pub(crate) async fn token(&self) -> Result<String, String> {
        let response = crate::auth::client_auth::paced(|| async {
            let form = self
                .auth
                .form(&self.client_id, &[("grant_type", "client_credentials")])
                .await?;
            self.http
                .post(format!("{}/protocol/openid-connect/token", self.issuer))
                .form(&form)
                .send()
                .await
                .map_err(|err| err.to_string())
        })
        .await?;
        if !response.status().is_success() {
            return Err(format!(
                "the realm refused the reconciler's client {}: {}",
                self.client_id,
                response.status()
            ));
        }
        let token: KcToken = response.json().await.map_err(|err| err.to_string())?;
        Ok(token.access_token)
    }

    pub(crate) async fn send(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        let response = crate::auth::client_auth::paced(|| {
            let mut request = self
                .http
                .request(method.clone(), format!("{}{path}", self.admin))
                .bearer_auth(token);
            if let Some(body) = body {
                request = request.json(body);
            }
            async { request.send().await.map_err(|err| err.to_string()) }
        })
        .await?;
        if !response.status().is_success() {
            // The path names a client id at most, never a secret; the body is not echoed.
            return Err(format!("{method} {}: {}", shown(path), response.status()));
        }
        if method == reqwest::Method::GET {
            return response
                .json()
                .await
                .map(Some)
                .map_err(|err| err.to_string());
        }
        Ok(None)
    }

    /// A POST whose answer is the value asked for, a regenerated client secret say: the body is
    /// handed back and never echoed into an error or a log line.
    pub(crate) async fn post_for(&self, token: &str, path: &str) -> Result<Value, String> {
        let response = crate::auth::client_auth::paced(|| {
            let request = self
                .http
                .post(format!("{}{path}", self.admin))
                .bearer_auth(token);
            async { request.send().await.map_err(|err| err.to_string()) }
        })
        .await?;
        if !response.status().is_success() {
            return Err(format!("POST {}: {}", shown(path), response.status()));
        }
        response
            .json()
            .await
            .map_err(|_| format!("POST {}: the realm's answer is not JSON", shown(path)))
    }

    pub(crate) async fn get<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
    ) -> Result<T, String> {
        let value = self
            .send(token, reqwest::Method::GET, path, None)
            .await?
            .unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|err| format!("GET {}: {err}", shown(path)))
    }

    /// Whether `path` names something the realm holds: a 404 is `false`, any other refusal an
    /// error naming the path.
    pub(crate) async fn exists(&self, token: &str, path: &str) -> Result<bool, String> {
        let response = crate::auth::client_auth::paced(|| {
            let request = self
                .http
                .get(format!("{}{path}", self.admin))
                .bearer_auth(token);
            async { request.send().await.map_err(|err| err.to_string()) }
        })
        .await?;
        match response.status() {
            status if status.is_success() => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            status => Err(format!("GET {}: {status}", shown(path))),
        }
    }

    /// The realm's client of one id, or `None`.
    pub(crate) async fn find(&self, token: &str, id: &str) -> Result<Option<Value>, String> {
        let found = self
            .send(
                token,
                reqwest::Method::GET,
                &format!("/clients?clientId={id}"),
                None,
            )
            .await?
            .unwrap_or(Value::Null);
        Ok(found.as_array().and_then(|list| list.first()).cloned())
    }

    /// Every optional scope unlinked from the client, reported in `outcome.drift` (T-6301). An
    /// App or MCP server login asks for `openid` alone, yet Keycloak links the realm's
    /// `defaultOptionalClientScopes` (`connector`, `offline_access`: the CIMD clients' list) to
    /// every client created without its own list, so a token of this client could otherwise
    /// carry the connectors' audience or an offline session.
    pub(crate) async fn drop_optional_scopes(
        &self,
        token: &str,
        uuid: &str,
        outcome: &mut ClientOutcome,
    ) -> Result<(), String> {
        let path = format!("/clients/{uuid}/optional-client-scopes");
        let linked: Vec<KcNamed> = self.get(token, &path).await?;
        for scope in &linked {
            self.send(
                token,
                reqwest::Method::DELETE,
                &format!("{path}/{}", scope.id),
                None,
            )
            .await?;
        }
        if !linked.is_empty() {
            let names: Vec<&str> = linked.iter().map(|scope| scope.name.as_str()).collect();
            outcome.drift.push(format!(
                "the client could ask for the scopes {}, which its login never needs; they were \
                 removed",
                names.join(", ")
            ));
        }
        Ok(())
    }

    /// The client's mappers brought to exactly `wanted` ([`audience_mapper`]s, and an App's
    /// [`groups_mapper`]): a missing one is added, a changed one written back, and every other
    /// mapper removed. `owner` names what decides them in the drift report, "the App" or "the
    /// account".
    pub(crate) async fn converge_mappers(
        &self,
        token: &str,
        uuid: &str,
        wanted: &[Value],
        owner: &str,
        outcome: &mut ClientOutcome,
    ) -> Result<(), String> {
        use reqwest::Method;
        let client = format!("/clients/{uuid}");
        // The client's mappers are the ones its owner names and nothing else (T-2857): FGAP bounds
        // which clients may be managed, never what a mapper writes into a token, so a hardcoded
        // claim, role or audience added by hand or through a leaked credential would mint tokens
        // its owner never asked for. Every other mapper goes first, so a foreign one that took one
        // of the wanted names cannot block writing ours.
        let mappers: Vec<Value> = self
            .get(token, &format!("{client}/protocol-mappers/models"))
            .await?;
        let (ours, others): (Vec<&Value>, Vec<&Value>) = mappers
            .iter()
            .partition(|mapper| allowed_mapper(mapper, wanted));
        for mapper in others {
            let Some(id) = mapper["id"].as_str() else {
                return Err(format!(
                    "the {} has no id, so it cannot be removed",
                    mapper_label(mapper)
                ));
            };
            self.send(
                token,
                Method::DELETE,
                &format!("{client}/protocol-mappers/models/{id}"),
                None,
            )
            .await?;
            let name = mapper["name"].as_str().unwrap_or_default();
            if mapper["protocolMapper"] == "oidc-audience-mapper"
                && name.starts_with(AUDIENCE_MAPPER)
            {
                outcome.drift.push(format!(
                    "the client put the audience {} in its tokens, which {owner} does not read; the mapper was removed",
                    name.trim_start_matches(AUDIENCE_MAPPER)
                ));
            } else {
                let label = mapper_label(mapper);
                outcome.drift.push(format!(
                    "the {label}, which {owner} does not declare, was removed"
                ));
                outcome.removed_mappers.push(label);
            }
        }
        for want in wanted {
            match ours.iter().find(|m| m["name"] == want["name"]) {
                None => {
                    self.send(
                        token,
                        Method::POST,
                        &format!("{client}/protocol-mappers/models"),
                        Some(want),
                    )
                    .await?;
                }
                Some(mapper) => {
                    let differs = want["config"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .any(|(key, value)| mapper["config"].get(key) != Some(value));
                    if differs {
                        let Some(id) = mapper["id"].as_str() else {
                            return Err(format!("the mapper {} has no id", want["name"]));
                        };
                        let mut body = want.clone();
                        body["id"] = json!(id);
                        self.send(
                            token,
                            Method::PUT,
                            &format!("{client}/protocol-mappers/models/{id}"),
                            Some(&body),
                        )
                        .await?;
                        let name = want["name"].as_str().unwrap_or_default();
                        outcome.drift.push(match name.strip_prefix(AUDIENCE_MAPPER) {
                            Some(audience) => format!(
                                "the audience mapper for {audience} was changed; it was written back"
                            ),
                            None => format!("the {name} mapper was changed; it was written back"),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

/// The realm's app clients, as the reconciler's own client may write them.
pub struct AppClientSync {
    admin: Admin,
    /// The credential that looks up groups and users and writes their role mappings
    /// (`portal-reconciler`: `manage-users`, `query-groups`). `admin` only reads who holds a role
    /// (T-3022); without it every call runs as `admin`, which then needs both grants.
    members: Option<Admin>,
    /// The apex the apps are served on.
    host: String,
    /// Where a run records the `app-*` clients it may not write, for the write doors (AP-114).
    foreign: Option<std::sync::Arc<super::foreign::ForeignNames>>,
}

impl AppClientSync {
    /// Makes each run record the realm's unmanaged `app-*` clients where the write doors read
    /// them.
    pub fn with_foreign(mut self, foreign: std::sync::Arc<super::foreign::ForeignNames>) -> Self {
        self.foreign = Some(foreign);
        self
    }

    /// `None` when the issuer is not a realm URL, because then there is nothing to manage.
    pub fn new(issuer: &str, client_id: String, auth: ClientAuth, host: String) -> Option<Self> {
        Some(Self {
            admin: Admin::new(issuer, client_id, auth)?,
            members: None,
            host,
            foreign: None,
        })
    }

    /// Looks up the App's groups and users and writes their role mappings as this client, in
    /// the same realm, instead of the client that manages the App clients (T-3022).
    pub fn with_members(mut self, client_id: String, auth: ClientAuth) -> Self {
        self.members = Admin::new(&self.admin.issuer, client_id, auth);
        self
    }

    fn members(&self) -> &Admin {
        self.members.as_ref().unwrap_or(&self.admin)
    }

    /// The id of the realm's group of this name; `None` while there is none.
    async fn group_id(
        &self,
        token: &str,
        index: &mut RealmIndex,
        name: &str,
    ) -> Result<Option<String>, String> {
        if index.groups.is_none() {
            let groups: Vec<KcNamed> = self
                .members()
                .get(token, "/groups?briefRepresentation=true&max=1000")
                .await?;
            index.groups = Some(groups.into_iter().map(|g| (g.name, g.id)).collect());
        }
        Ok(index
            .groups
            .as_ref()
            .and_then(|groups| groups.get(name))
            .cloned())
    }

    /// The id of the realm's user a role member names: their Keycloak user id (ADR-N-061) when
    /// the realm has it, or, for a manifest written before, the user of that e-mail or username;
    /// `None` while the realm has nobody under it.
    async fn user_id(
        &self,
        token: &str,
        index: &mut RealmIndex,
        user: &str,
    ) -> Result<Option<String>, String> {
        if let Some(known) = index.users.get(user) {
            return Ok(known.clone());
        }
        if crate::permissions::is_person_id(user) {
            let found = self
                .members()
                .exists(token, &format!("/users/{user}"))
                .await?;
            let id = found.then(|| user.to_owned());
            index.users.insert(user.to_owned(), id.clone());
            return Ok(id);
        }
        let field = if user.contains('@') {
            "email"
        } else {
            "username"
        };
        let found: Vec<KcUserRef> = self
            .members()
            .get(
                token,
                &format!(
                    "/users?exact=true&{field}={}",
                    super::groups::urlencoding(user)
                ),
            )
            .await?;
        let id = found.into_iter().find(|u| u.is(user)).map(|u| u.id);
        index.users.insert(user.to_owned(), id.clone());
        Ok(id)
    }

    /// The App's roles and who holds each, brought to the manifest (AP-113).
    /// What the realm held and the manifest does not say goes, and is reported as drift.
    ///
    /// `token` is the App-client manager's: it writes the client's roles and reads who holds
    /// each. `members` is the member credential's: every group or user lookup and every mapping
    /// written or removed carries it.
    async fn converge_grants(
        &self,
        token: &str,
        members: &str,
        uuid: &str,
        spec: &AppSpec,
        index: &mut RealmIndex,
        outcome: &mut ClientOutcome,
    ) -> Result<(), String> {
        use reqwest::Method;
        let client = format!("/clients/{uuid}");
        let wanted = holders(spec);

        let mut held: Vec<KcNamed> = self.admin.get(token, &format!("{client}/roles")).await?;
        for role in held.iter().filter(|role| !wanted.contains_key(&role.name)) {
            self.admin
                .send(
                    token,
                    Method::DELETE,
                    &format!("{client}/roles/{}", role_segment(&role.name)?),
                    None,
                )
                .await?;
            outcome.drift.push(format!(
                "the client held the role {}, which the App does not declare; it was removed",
                role.name
            ));
        }
        let missing: Vec<&String> = wanted
            .keys()
            .filter(|name| !held.iter().any(|role| &role.name == *name))
            .collect();
        for name in &missing {
            self.admin
                .send(
                    token,
                    Method::POST,
                    &format!("{client}/roles"),
                    Some(&json!({ "name": name })),
                )
                .await?;
        }
        if !missing.is_empty() {
            held = self.admin.get(token, &format!("{client}/roles")).await?;
        }

        for (name, subjects) in &wanted {
            let role = held
                .iter()
                .find(|role| &role.name == name)
                .ok_or_else(|| format!("the role {name} was created and cannot be read back"))?;
            let mapping = json!([{ "id": role.id, "name": role.name }]);

            let groups: Vec<KcNamed> = self
                .admin
                .get(
                    token,
                    &format!(
                        "{client}/roles/{}/groups?briefRepresentation=true&max=1000",
                        role_segment(name)?
                    ),
                )
                .await?;
            let users: Vec<KcUserRef> = self
                .admin
                .get(
                    token,
                    &format!("{client}/roles/{}/users?max=1000", role_segment(name)?),
                )
                .await?;
            for group in &groups {
                if !subjects.contains(&Holder::Group(group.name.clone())) {
                    self.members()
                        .send(
                            members,
                            Method::DELETE,
                            &format!("/groups/{}/role-mappings/clients/{uuid}", group.id),
                            Some(&mapping),
                        )
                        .await?;
                    outcome.drift.push(format!(
                        "the group {} held the role {name} and the App does not give it; the mapping was removed",
                        group.name
                    ));
                }
            }
            for user in &users {
                let named = subjects
                    .iter()
                    .any(|holder| matches!(holder, Holder::User(u) if user.is(u)));
                if !named {
                    self.members()
                        .send(
                            members,
                            Method::DELETE,
                            &format!("/users/{}/role-mappings/clients/{uuid}", user.id),
                            Some(&mapping),
                        )
                        .await?;
                    outcome.drift.push(format!(
                        "the user {} held the role {name} and the App does not give it; the mapping was removed",
                        user.email.as_deref().unwrap_or(&user.username)
                    ));
                }
            }
            for holder in subjects {
                let (path, id) = match holder {
                    Holder::Group(group) => {
                        if groups.iter().any(|g| &g.name == group) {
                            continue;
                        }
                        ("groups", self.group_id(members, index, group).await?)
                    }
                    Holder::User(user) => {
                        if users.iter().any(|u| u.is(user)) {
                            continue;
                        }
                        ("users", self.user_id(members, index, user).await?)
                    }
                };
                match id {
                    Some(id) => {
                        self.members()
                            .send(
                                members,
                                Method::POST,
                                &format!("/{path}/{id}/role-mappings/clients/{uuid}"),
                                Some(&mapping),
                            )
                            .await?;
                    }
                    None => outcome.warnings.push(format!(
                        "the realm has no {holder} yet, so the role {name} is mapped once it does"
                    )),
                }
            }
        }
        Ok(())
    }

    async fn secret(&self, token: &str, uuid: &str) -> Result<ClientSecret, String> {
        let value = self
            .admin
            .send(
                token,
                reqwest::Method::GET,
                &format!("/clients/{uuid}/client-secret"),
                None,
            )
            .await?
            .unwrap_or(Value::Null);
        value
            .get("value")
            .and_then(Value::as_str)
            .filter(|secret| !secret.is_empty())
            .map(|secret| ClientSecret(secret.to_owned()))
            .ok_or_else(|| "the realm answered no secret for the client".to_owned())
    }

    /// Brings every published App's client to what [`desired`] says, deletes the managed clients
    /// no published App names, and answers the outcomes and the secrets.
    pub async fn converge(&self, mirror: &Mirror) -> ClientRun {
        let mut run = ClientRun::default();
        let token = match self.admin.token().await {
            Ok(token) => token,
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(err);
                run.outcomes.push(outcome);
                return run;
            }
        };

        // A refused member credential fails each App's roles, never its client or audiences.
        let members = match self.members.as_ref() {
            Some(members) => members.token().await,
            None => Ok(token.clone()),
        };

        let apps = published(mirror, &self.host);
        let wanted: BTreeSet<String> = apps.iter().map(|app| client_id(&app.name)).collect();
        let mut index = RealmIndex::default();
        for app in &apps {
            let audiences: Vec<String> = std::iter::once(client_id(&app.name))
                .chain(app.spec.iter().flat_map(|spec| {
                    crate::apps::static_host::served_endpoints(
                        mirror,
                        &app.project,
                        &app.name,
                        spec,
                    )
                    .into_iter()
                    .map(|endpoint| endpoint.slug)
                }))
                .collect();
            let (outcome, secret) = self
                .converge_one(&token, members.as_deref(), app, &audiences, &mut index)
                .await;
            if let Some(secret) = secret {
                run.secrets.insert(app.name.clone(), secret);
            }
            run.outcomes.push(outcome);
        }

        // A managed app client no published App names is this wave's to remove (AP-111).
        match self
            .admin
            .send(
                &token,
                reqwest::Method::GET,
                "/clients?search=true&clientId=app-&max=1000",
                None,
            )
            .await
        {
            Ok(listed) => {
                if let Some(foreign) = self.foreign.as_ref() {
                    foreign.set_clients(
                        listed
                            .as_ref()
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter(|client| !managed(client))
                            .filter_map(|client| client.get("clientId").and_then(Value::as_str))
                            .filter(|id| id.starts_with("app-"))
                            .map(str::to_owned)
                            .collect(),
                    );
                }
                for client in listed
                    .as_ref()
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(id) = client.get("clientId").and_then(Value::as_str) else {
                        continue;
                    };
                    if !id.starts_with("app-") || !managed(client) || wanted.contains(id) {
                        continue;
                    }
                    let app = id.trim_start_matches("app-");
                    let mut outcome = ClientOutcome::of(app);
                    match client.get("id").and_then(Value::as_str) {
                        Some(uuid) => match self
                            .admin
                            .send(
                                &token,
                                reqwest::Method::DELETE,
                                &format!("/clients/{uuid}"),
                                None,
                            )
                            .await
                        {
                            Ok(_) => outcome.drift.push(
                                "the App is no longer published, so its client was removed"
                                    .to_owned(),
                            ),
                            Err(err) => outcome.error = Some(err),
                        },
                        None => {
                            outcome.error =
                                Some("the realm listed a client without an id".to_owned())
                        }
                    }
                    run.outcomes.push(outcome);
                }
            }
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(format!("listing the app clients: {err}"));
                run.outcomes.push(outcome);
            }
        }
        run
    }

    async fn converge_one(
        &self,
        token: &str,
        members: Result<&str, &String>,
        app: &PublishedApp,
        audiences: &[String],
        index: &mut RealmIndex,
    ) -> (ClientOutcome, Option<ClientSecret>) {
        let mut outcome = ClientOutcome::of(&app.name);
        let id = client_id(&app.name);
        let want = desired(
            &app.project,
            &app.name,
            &app.title,
            &self.host,
            &app.hostnames,
        );

        let held = match self.admin.find(token, &id).await {
            Ok(held) => held,
            Err(err) => {
                outcome.error = Some(err);
                return (outcome, None);
            }
        };
        let uuid = match held {
            Some(client) if !managed(&client) => {
                outcome.error = Some(format!(
                    "the realm holds a client {id} this platform did not create; it is left \
                     alone and the App has no login of its own until it is renamed or removed \
                     (AP-114)"
                ));
                return (outcome, None);
            }
            Some(client) => {
                let Some(uuid) = client.get("id").and_then(Value::as_str).map(str::to_owned) else {
                    outcome.error = Some(format!("the realm answered {id} without an id"));
                    return (outcome, None);
                };
                let found = drift(&want, &client);
                if !found.is_empty() {
                    if let Err(err) = self
                        .admin
                        .send(
                            token,
                            reqwest::Method::PUT,
                            &format!("/clients/{uuid}"),
                            Some(&want),
                        )
                        .await
                    {
                        outcome.error = Some(err);
                        return (outcome, None);
                    }
                    outcome.drift = found;
                }
                uuid
            }
            None => {
                if let Err(err) = self
                    .admin
                    .send(token, reqwest::Method::POST, "/clients", Some(&want))
                    .await
                {
                    outcome.error = Some(err);
                    return (outcome, None);
                }
                match self.admin.find(token, &id).await {
                    Ok(Some(client)) => match client.get("id").and_then(Value::as_str) {
                        Some(uuid) => uuid.to_owned(),
                        None => {
                            outcome.error = Some(format!("the realm answered {id} without an id"));
                            return (outcome, None);
                        }
                    },
                    Ok(None) => {
                        outcome.error = Some(format!("{id} was created and cannot be read back"));
                        return (outcome, None);
                    }
                    Err(err) => {
                        outcome.error = Some(err);
                        return (outcome, None);
                    }
                }
            }
        };
        // The login works without the roles, so a failure here keeps the secret on the edge.
        // The audiences are what that login's token reaches, so they are brought in place
        // whatever the roles did: a refused role mapping once left a login whose token named no
        // endpoint, and every read the App made was a 401 (T-2965).
        let failed: Vec<String> = match &app.spec {
            Some(spec) => {
                let grants = match members {
                    Ok(members) => {
                        self.converge_grants(token, members, &uuid, spec, index, &mut outcome)
                            .await
                    }
                    Err(err) => Err(err.clone()),
                };
                let reach = self
                    .admin
                    .converge_mappers(
                        token,
                        &uuid,
                        &app_mappers(audiences),
                        "the App",
                        &mut outcome,
                    )
                    .await;
                let scopes = self
                    .admin
                    .drop_optional_scopes(token, &uuid, &mut outcome)
                    .await;
                [
                    grants.err().map(|err| format!("roles of {id}: {err}")),
                    reach.err().map(|err| format!("audiences of {id}: {err}")),
                    scopes.err().map(|err| format!("scopes of {id}: {err}")),
                ]
                .into_iter()
                .flatten()
                .collect()
            }
            None => vec![format!(
                "roles of {id}: the App manifest does not parse, so its roles were not written"
            )],
        };
        if !failed.is_empty() {
            outcome.error = Some(failed.join("; "));
        }
        match self.secret(token, &uuid).await {
            Ok(secret) => (outcome, Some(secret)),
            Err(err) => {
                outcome.error = Some(err);
                (outcome, None)
            }
        }
    }
}

/// The mappers a run removed, on the App itself (T-2857): a condition `ClientSynced` with the
/// reason `MapperRemoved` naming each by type and name, so the owner sees that something wrote
/// into the App's client and what it was. The App's other conditions stay as they are.
pub fn record(mirror: &Mirror, outcomes: &[ClientOutcome]) {
    for outcome in outcomes {
        if outcome.removed_mappers.is_empty() {
            continue;
        }
        let Some(mut envelope) =
            mirror.find(|env| env.kind == "App" && env.metadata.name == outcome.app)
        else {
            continue;
        };
        let Some(status) = envelope.status.as_mut() else {
            continue;
        };
        status
            .conditions
            .retain(|condition| condition.r#type != "ClientSynced");
        status.conditions.push(super::streams::make_condition(
            "ClientSynced",
            "True",
            "MapperRemoved",
            &format!(
                "removed from the App's login client: {}",
                outcome.removed_mappers.join("; ")
            ),
        ));
        mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_admin_base_is_the_realm_url_with_admin_in_front_of_realms() {
        let auth = || ClientAuth::Secret("s".into());
        for issuer in [
            "https://id.example.sk/realms/bb",
            "https://id.example.sk/realms/bb/",
        ] {
            let admin = Admin::new(issuer, "c".into(), auth()).expect("a realm URL");
            assert_eq!(admin.admin, "https://id.example.sk/admin/realms/bb");
        }
        assert!(Admin::new("https://id.example.sk/", "c".into(), auth()).is_none());
    }

    #[test]
    fn a_role_name_stays_one_segment_under_its_client() {
        assert_eq!(role_segment("reader").as_deref(), Ok("reader"));
        assert_eq!(
            role_segment("../../x?y#z").as_deref(),
            Ok("..%2F..%2Fx%3Fy%23z")
        );
        for refused in ["", ".", ".."] {
            assert!(role_segment(refused).is_err(), "{refused:?}");
        }
    }

    #[tokio::test]
    async fn a_refused_lookup_by_email_names_no_email() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let admin = Admin::new(
            &format!("{}/realms/r", server.uri()),
            "c".into(),
            ClientAuth::Secret("s".into()),
        )
        .expect("a realm URL");
        let said = admin
            .get::<Value>("t", "/users?exact=true&email=ana%40example.sk")
            .await
            .expect_err("refused");
        assert_eq!(said, "GET /users: 500 Internal Server Error");
    }

    /// T-6301: a client the Portal creates inherits the realm's optional scopes (the CIMD
    /// clients' `connector` and `offline_access`); each pass unlinks every one and says so.
    #[tokio::test]
    async fn every_inherited_optional_scope_is_unlinked_and_reported() {
        use wiremock::matchers::{method, path};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .and(path("/admin/realms/r/clients/u1/optional-client-scopes"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!([
                { "id": "s-con", "name": "connector" },
                { "id": "s-off", "name": "offline_access" },
            ])))
            .mount(&server)
            .await;
        for id in ["s-con", "s-off"] {
            wiremock::Mock::given(method("DELETE"))
                .and(path(format!(
                    "/admin/realms/r/clients/u1/optional-client-scopes/{id}"
                )))
                .respond_with(wiremock::ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;
        }
        let admin = Admin::new(
            &format!("{}/realms/r", server.uri()),
            "c".into(),
            ClientAuth::Secret("s".into()),
        )
        .expect("a realm URL");
        let mut outcome = ClientOutcome::of("app-x");
        admin
            .drop_optional_scopes("t", "u1", &mut outcome)
            .await
            .expect("unlinked");
        assert_eq!(outcome.drift.len(), 1, "{:?}", outcome.drift);
        assert!(
            outcome.drift[0].contains("connector, offline_access"),
            "{:?}",
            outcome.drift
        );

        // Nothing linked: nothing deleted, nothing reported.
        let empty = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&empty)
            .await;
        wiremock::Mock::given(method("DELETE"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .expect(0)
            .mount(&empty)
            .await;
        let admin = Admin::new(
            &format!("{}/realms/r", empty.uri()),
            "c".into(),
            ClientAuth::Secret("s".into()),
        )
        .expect("a realm URL");
        let mut outcome = ClientOutcome::of("app-x");
        admin
            .drop_optional_scopes("t", "u1", &mut outcome)
            .await
            .expect("nothing to unlink");
        assert!(outcome.drift.is_empty(), "{:?}", outcome.drift);
    }

    #[test]
    fn a_kc_token_debug_never_shows_the_admin_token() {
        let token = KcToken {
            access_token: "s3cr3t-test".into(),
        };
        let text = format!("{token:?}");
        assert!(
            !text.contains("s3cr3t-test") && text.contains("[redacted]"),
            "{text}"
        );
    }

    /// PF-90: the login page reads "Sign in to {client name} · {organization}", so the client is
    /// named after the App's title, and an App without one after its name, never `project/app`.
    #[test]
    fn the_client_is_named_after_the_apps_title() {
        let titled = envelope_of(json!({ "name": "air-quality", "namespace": "helsinki",
            "title": { "fi": "Ilmanlaatu", "en": "Air quality" } }));
        assert_eq!(title_of(&titled), "Air quality");
        let plain =
            envelope_of(json!({ "name": "bikes", "namespace": "helsinki", "title": "City bikes" }));
        assert_eq!(title_of(&plain), "City bikes");
        let blank = envelope_of(json!({ "name": "bikes", "namespace": "helsinki", "title": "  " }));
        assert_eq!(title_of(&blank), "bikes");
        let untitled = envelope_of(json!({ "name": "bikes", "namespace": "helsinki" }));
        assert_eq!(title_of(&untitled), "bikes");
        assert_eq!(
            desired("helsinki", "bikes", "City bikes", "city.example", &[])["name"],
            "City bikes"
        );
    }

    fn envelope_of(metadata: Value) -> ResourceEnvelope {
        serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "App", "metadata": metadata, "spec": {},
        }))
        .expect("an envelope")
    }

    #[test]
    fn a_secret_never_shows_in_debug() {
        let secret = ClientSecret("hunter2-value".to_owned());
        assert_eq!(format!("{secret:?}"), "ClientSecret(redacted)");
        assert_eq!(secret.expose(), "hunter2-value");
    }

    /// AP-172: a routed hostname is one more redirect URI and web origin of the App's own
    /// client, and nothing else changes.
    #[test]
    fn the_client_redirects_to_its_hostnames_too() {
        let want = desired(
            "helsinki",
            "bikes",
            "Bikes",
            "city.example",
            &["bikes.hel.fi".to_owned()],
        );
        assert_eq!(
            want["redirectUris"],
            json!([
                "https://bikes.apps.city.example/*",
                "https://bikes.hel.fi/*"
            ])
        );
        assert_eq!(
            want["webOrigins"],
            json!(["https://bikes.apps.city.example", "https://bikes.hel.fi"])
        );
        assert_eq!(
            want["attributes"]["post.logout.redirect.uris"],
            "https://bikes.apps.city.example/*##https://bikes.hel.fi/*"
        );
        assert_eq!(want["clientId"], "app-bikes");
    }

    #[test]
    fn the_client_redirects_only_to_its_own_host_and_has_no_other_flow() {
        let want = desired("helsinki", "bikes", "Bikes", "city.example", &[]);
        assert_eq!(want["clientId"], "app-bikes");
        // Its own host alone (AP-133): not the apex, not another App's host.
        assert_eq!(
            want["redirectUris"],
            json!(["https://bikes.apps.city.example/*"])
        );
        assert_eq!(
            want["attributes"]["post.logout.redirect.uris"],
            "https://bikes.apps.city.example/*"
        );
        assert_eq!(
            want["webOrigins"],
            json!(["https://bikes.apps.city.example"])
        );
        assert_eq!(want["directAccessGrantsEnabled"], false);
        assert_eq!(want["serviceAccountsEnabled"], false);
        assert_eq!(want["implicitFlowEnabled"], false);
        assert_eq!(want["publicClient"], false);
        assert_eq!(want["attributes"]["pkce.code.challenge.method"], "S256");
        assert_eq!(want["attributes"]["access.token.lifespan"], "60");
        assert_eq!(want["attributes"][MANAGED_BY], MANAGED_VALUE);
        assert_eq!(want["attributes"][APP_ATTRIBUTE], "helsinki/bikes");
    }

    #[test]
    fn drift_names_only_the_fields_the_app_sets() {
        let want = desired("helsinki", "bikes", "Bikes", "city.example", &[]);
        let mut held = want.clone();
        held["id"] = json!("uuid-1");
        held["surrogateAuthRequired"] = json!(false);
        assert!(
            drift(&want, &held).is_empty(),
            "fields Keycloak adds are its own"
        );

        held["directAccessGrantsEnabled"] = json!(true);
        held["redirectUris"] = json!(["*"]);
        held["attributes"]["pkce.code.challenge.method"] = json!("plain");
        let found = drift(&want, &held);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found
            .iter()
            .any(|d| d.starts_with("directAccessGrantsEnabled")));
        assert!(found.iter().any(|d| d.starts_with("redirectUris")));
        assert!(found
            .iter()
            .any(|d| d.contains("pkce.code.challenge.method")));
    }

    #[test]
    fn drift_ignores_the_order_of_a_set_valued_field() {
        let mut want = desired("helsinki", "bikes", "Bikes", "city.example", &[]);
        want["redirectUris"] = json!([
            "https://a.example/cb",
            "https://b.example/cb",
            "https://c.example/cb"
        ]);
        want["webOrigins"] = json!(["https://a.example", "https://b.example"]);
        let mut held = want.clone();
        held["redirectUris"] = json!([
            "https://c.example/cb",
            "https://a.example/cb",
            "https://b.example/cb"
        ]);
        held["webOrigins"] = json!(["https://b.example", "https://a.example"]);
        assert_eq!(drift(&want, &held), Vec::<String>::new());
    }

    #[test]
    fn drift_names_a_missing_redirect_uri() {
        let mut want = desired("helsinki", "bikes", "Bikes", "city.example", &[]);
        want["redirectUris"] = json!(["https://a.example/cb", "https://b.example/cb"]);
        let mut held = want.clone();
        held["redirectUris"] = json!(["https://b.example/cb"]);
        let found = drift(&want, &held);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("redirectUris"), "{found:?}");

        held["redirectUris"] = json!(["https://b.example/cb", "https://a.example/cb", "*"]);
        assert_eq!(drift(&want, &held).len(), 1, "an added wildcard is a drift");
    }

    #[test]
    fn holders_follow_access_and_an_undeclared_role_grants_nothing() {
        let spec: AppSpec = serde_json::from_value(json!({
            "kind": "static", "source": { "path": "." }, "build": {}, "visibility": "project",
            "dataNeeds": [],
            "roles": [{ "name": "viewer" }, { "name": "steward" }],
            "access": [
                { "role": "viewer", "subjects": [{ "group": "stewards" }, { "user": "Jana@Hel.fi" }] },
                { "role": "ghost", "subjects": [{ "group": "everyone" }] },
            ],
        }))
        .expect("an App spec");
        let held = holders(&spec);
        assert_eq!(held.len(), 2, "every declared role, and no other");
        assert_eq!(
            held["viewer"],
            BTreeSet::from([
                Holder::Group("stewards".into()),
                Holder::User("jana@hel.fi".into())
            ])
        );
        assert!(held["steward"].is_empty());
        assert!(!held.contains_key("ghost"));
    }

    #[test]
    fn a_client_without_the_attribute_is_not_managed() {
        assert!(!managed(&json!({ "clientId": "app-bikes" })));
        assert!(!managed(
            &json!({ "attributes": { MANAGED_BY: "someone" } })
        ));
        assert!(managed(
            &json!({ "attributes": { MANAGED_BY: MANAGED_VALUE } })
        ));
    }
}
