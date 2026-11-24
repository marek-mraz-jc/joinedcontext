//! The realm of an Organization with a sign-in host of its own (`hosts.idm`, T-7001, T-7133):
//! `{scheme}://{idm host}/realms/{organization}` (`JC_PORTAL_ORGANIZATION_REALMS`, jc fills it).
//!
//! On that Organization's Portal host the Portal signs a person in through it, refreshes and ends
//! the session there, and takes a bearer token of it. Nowhere else: a session of the realm opens
//! on its Organization's host alone (`session::open`), and its tokens are checked only on that
//! host, so a token of hel's realm is a foreign issuer's on bbsk's host and on the installation's.
//! The installation realm (`JC_OIDC_ISSUER`) stays everywhere else and beside it on the
//! Organization's host: operators sign in on `login_host`, and a person not moved into the
//! Organization's realm yet (T-7128) still holds an installation session.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use url::Url;

use crate::auth::bearer::BearerVerifier;
use crate::auth::oidc::OidcClient;
use crate::auth::session::Identity;
use crate::config::OidcConfig;
use crate::error::ApiError;
use crate::organization_host::OrganizationHost;
use crate::state::AppState;

/// One row of `JC_PORTAL_ORGANIZATION_REALMS`, the gateway's `JC_OIDC_ORGANIZATION_REALMS` shape.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmSetting {
    /// The `Organization` manifest's name, also the realm's.
    pub organization: String,
    /// The realm's public issuer, e.g. `https://idm.city.example/realms/city`.
    pub issuer: Url,
    /// Where the Portal reads the realm's keys: the issuer's path under the realm's back channel,
    /// e.g. `http://idm.city.example/realms/city/protocol/openid-connect/certs` dialed at jc's
    /// listener (`JC_PORTAL_REACH`). Its origin is where the Portal posts for tokens.
    pub jwks_url: Url,
}

const CERTS: &str = "/protocol/openid-connect/certs";

fn trimmed(url: &Url) -> &str {
    url.as_str().trim_end_matches('/')
}

fn plain_http(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

/// `JC_PORTAL_ORGANIZATION_REALMS`: a JSON array of [`RealmSetting`]. Each Organization once,
/// with a Portal host of its own; each issuer once, never the installation's; the keys under the
/// issuer's own path. Empty or unset: no Organization signs in through a realm of its own.
pub fn parse(
    raw: &str,
    installation: Option<&OidcConfig>,
    hosts: &[OrganizationHost],
) -> Result<Vec<RealmSetting>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let installation = installation.ok_or(
        "an Organization's realm needs the installation's realm (JC_OIDC_ISSUER) beside it",
    )?;
    let settings: Vec<RealmSetting> =
        serde_json::from_str(raw).map_err(|e| format!("not a list of realms: {e}"))?;
    let mut seen: Vec<&RealmSetting> = Vec::new();
    for s in &settings {
        let name = &s.organization;
        if !hosts.iter().any(|h| &h.organization == name) {
            return Err(format!(
                "{name}: no Portal host in JC_PORTAL_ORGANIZATION_HOSTS, so nobody signs in there"
            ));
        }
        if !plain_http(&s.issuer) || !plain_http(&s.jwks_url) {
            return Err(format!(
                "{name}: the issuer and the key URL are http(s) URLs without credentials or query"
            ));
        }
        if trimmed(&s.issuer) == trimmed(&installation.issuer) {
            return Err(format!("{name}: the issuer is the installation's own"));
        }
        let keys_at = format!("{}{CERTS}", s.issuer.path().trim_end_matches('/'));
        if s.jwks_url.path() != keys_at {
            return Err(format!("{name}: the key URL's path is not {keys_at}"));
        }
        if seen.iter().any(|o| &o.organization == name) {
            return Err(format!("{name}: named twice"));
        }
        if seen
            .iter()
            .any(|o| trimmed(&o.issuer) == trimmed(&s.issuer))
        {
            return Err(format!("{name}: the issuer is another Organization's"));
        }
        seen.push(s);
    }
    Ok(settings)
}

/// An Organization's realm at run time: its sign-in client, discovered on first use, and its
/// bearer verifier.
pub struct OrganizationRealm {
    pub organization: String,
    issuer: String,
    config: OidcConfig,
    redirect_uri: String,
    client: tokio::sync::OnceCell<Arc<OidcClient>>,
    pub bearer: BearerVerifier,
    /// The realm's people, as its own `portal-reconciler` manages them (T-7142); `None` without
    /// the admin client (`JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID`).
    pub people: Option<Arc<crate::people::People>>,
    /// The realm's issuer path at its back channel, where the Portal's admin clients post
    /// (`http://idm.city.example/realms/city`).
    pub backchannel: String,
    /// What it was built from: a set replaced at run time keeps a realm whose setting is equal.
    setting: RealmSetting,
}

/// The Organizations' own realms by name (T-7180): built at start, replaced whenever jc's host
/// registry serves another set of sign-in hosts ([`AppState::replace_organization_realms`]).
/// Every clone holds the same set; a request reads the set current when it asks.
#[derive(Clone, Default)]
pub struct OrganizationRealms(Arc<RwLock<Arc<RealmMap>>>);

type RealmMap = BTreeMap<String, Arc<OrganizationRealm>>;

impl OrganizationRealms {
    pub fn new(realms: RealmMap) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(realms))))
    }

    /// The set held now.
    pub fn current(&self) -> Arc<RealmMap> {
        match self.0.read() {
            Ok(held) => Arc::clone(&held),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub fn get(&self, organization: &str) -> Option<Arc<OrganizationRealm>> {
        self.current().get(organization).cloned()
    }

    pub fn values(&self) -> Vec<Arc<OrganizationRealm>> {
        self.current().values().cloned().collect()
    }

    pub fn keys(&self) -> Vec<String> {
        self.current().keys().cloned().collect()
    }

    /// Drops every realm of an Organization not in `organizations`.
    pub(crate) fn retain(&self, organizations: &[String]) {
        let next: RealmMap = self
            .current()
            .iter()
            .filter(|(name, _)| organizations.contains(name))
            .map(|(name, realm)| (name.clone(), Arc::clone(realm)))
            .collect();
        self.store(next);
    }

    fn store(&self, next: RealmMap) {
        let next = Arc::new(next);
        match self.0.write() {
            Ok(mut held) => *held = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
    }
}

impl AppState {
    /// Makes `raw` (a `JC_PORTAL_ORGANIZATION_REALMS` list, checked as at start) the
    /// Organizations' own realms (T-7180). A realm left out signs nobody in from the next request
    /// on and its sessions end at their next refresh; a realm whose setting is unchanged keeps its
    /// client and keys; a new realm's host is dialed at jc's listener `reach`. A set that fails
    /// the check changes nothing. Returns how many realms are held.
    pub fn replace_organization_realms(
        &self,
        raw: &str,
        reach: Option<std::net::SocketAddr>,
    ) -> Result<usize, String> {
        let settings = parse(
            raw,
            self.config.oidc.as_ref(),
            &self.organization_hosts.current(),
        )?;
        if let Some(addr) = reach {
            for s in &settings {
                if let Some(host) = s.jwks_url.host_str() {
                    crate::reach::add(host, addr);
                }
            }
        }
        let held = self.organization_realms.current();
        let next: RealmMap = settings
            .iter()
            .filter_map(|s| match held.get(&s.organization) {
                Some(realm) if realm.setting == *s => {
                    Some((s.organization.clone(), Arc::clone(realm)))
                }
                _ => crate::state::organization_realm(
                    &self.config,
                    &self.organization_hosts.current(),
                    s,
                ),
            })
            .collect();
        let count = next.len();
        self.organization_realms.store(next);
        Ok(count)
    }
}

/// `client`'s proof in `realm`: a client secret becomes the realm's own; a federated assertion
/// stays, as it names no secret.
fn own_auth(
    installation: &crate::auth::client_auth::ClientAuth,
    realm: &str,
    client: &str,
) -> crate::auth::client_auth::ClientAuth {
    use crate::auth::client_auth::ClientAuth;
    match installation {
        ClientAuth::Secret(secret) => ClientAuth::Secret(
            jc_core::Secret::new(secret.as_str())
                .for_realm(realm, client)
                .expose()
                .to_owned(),
        ),
        other => other.clone(),
    }
}

impl OrganizationRealm {
    /// The realm of `setting`, signing in as the installation's Portal client with the redirect
    /// URI of the Organization's host `base`, and managing its people as the installation's
    /// admin client `admin` of the same name. A client secret is the realm's own, derived from
    /// the installation's as jc's kaisatsu writes it into the realm (S1-04,
    /// `setup::organization_realm_env`): a secret read in one realm signs in nowhere else.
    pub fn new(
        setting: &RealmSetting,
        installation: &OidcConfig,
        base: &Url,
        admin: Option<&(String, crate::auth::client_auth::ClientAuth)>,
    ) -> Self {
        let mut origin = setting.jwks_url.clone();
        origin.set_path("/");
        let backchannel = format!(
            "{}{}",
            origin.as_str().trim_end_matches('/'),
            setting.issuer.path().trim_end_matches('/')
        );
        Self {
            people: admin.and_then(|(id, auth)| {
                let auth = own_auth(auth, &setting.organization, id);
                crate::people::People::new(&backchannel, id.clone(), auth).map(Arc::new)
            }),
            setting: setting.clone(),
            backchannel,
            organization: setting.organization.clone(),
            issuer: trimmed(&setting.issuer).to_owned(),
            config: OidcConfig {
                issuer: setting.issuer.clone(),
                client_id: installation.client_id.clone(),
                auth: own_auth(
                    &installation.auth,
                    &setting.organization,
                    &installation.client_id,
                ),
                extra_ca_pem: installation.extra_ca_pem.clone(),
                admin_url: Some(origin),
            },
            redirect_uri: crate::config::Config::redirect_uri_at(base),
            client: tokio::sync::OnceCell::new(),
            bearer: BearerVerifier::reached(
                &setting.issuer,
                &installation.client_id,
                setting.jwks_url.clone(),
            ),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The installation's admin client `(id, auth)` as it signs in to this realm: same id,
    /// this realm's own secret (S1-04). Every syncer that writes the realm takes it, or the
    /// realm answers 401 (T-7288).
    pub fn admin_auth(
        &self,
        (id, auth): &(String, crate::auth::client_auth::ClientAuth),
    ) -> (String, crate::auth::client_auth::ClientAuth) {
        (id.clone(), own_auth(auth, &self.organization, id))
    }

    /// The realm as a sign-in configuration: its public issuer, the Portal's client and the
    /// back channel the Portal posts to.
    pub fn oidc(&self) -> &OidcConfig {
        &self.config
    }

    /// The sign-in client, discovered the first time a person signs in on the Organization's
    /// host: a realm that does not answer yet costs that sign-in a 503, never the Portal's start.
    pub async fn client(&self) -> Result<Arc<OidcClient>, ApiError> {
        self.client
            .get_or_try_init(|| async {
                OidcClient::discover(&self.config, &self.redirect_uri)
                    .await
                    .map(Arc::new)
            })
            .await
            .cloned()
            .map_err(|e| {
                tracing::warn!(organization = %self.organization, error = %e, "the Organization's realm did not answer discovery");
                ApiError::Unavailable(format!(
                    "the sign-in of {} does not answer; try again in a minute",
                    self.organization
                ))
            })
    }
}

/// The Organization the running request is pinned to, when it signs in through a realm of its
/// own.
pub fn here(state: &AppState) -> Option<Arc<OrganizationRealm>> {
    let organization = crate::organization_host::organization()?;
    state.organization_realms.get(&organization)
}

/// The realm App `app` signs its people in at (T-7165): its project's Organization's own, when
/// that Organization has one; `None`: the installation realm. Read off the App's manifest, never
/// off the request's host.
pub fn of_app(state: &AppState, app: &str) -> Option<Arc<OrganizationRealm>> {
    let project = state
        .mirror
        .find(|env| env.kind == "App" && env.metadata.name == app)?
        .metadata
        .namespace?;
    let organization = crate::permissions::organization_of(&state.mirror, &project)?;
    state.organization_realms.get(&organization)
}

/// The sign-in configuration of App `app`'s realm ([`of_app`]), else the installation's.
pub fn app_oidc(state: &AppState, app: &str) -> Option<OidcConfig> {
    match of_app(state, app) {
        Some(realm) => Some(realm.oidc().clone()),
        None => state.config.oidc.clone(),
    }
}

/// `token` as an access token of App `app`'s own client, verified by the App's realm alone
/// ([`of_app`]): a token of the installation or of another Organization is `401` on that App.
pub async fn verify_app_token(
    state: &AppState,
    app: &str,
    token: &str,
) -> Result<(crate::auth::session::Session, Vec<String>), ApiError> {
    let client = crate::reconciler::app_clients::client_id(app);
    match of_app(state, app) {
        Some(realm) => realm.bearer.verify_app_token(token, &client).await,
        None => {
            state
                .bearer
                .as_ref()
                .ok_or(ApiError::Unauthorized)?
                .verify_app_token(token, &client)
                .await
        }
    }
}

/// The Organization realm that checks `token` on the running request: the pinned Organization's,
/// when the token's `iss` names it. None: the installation realm checks it, so another
/// Organization's realm is a foreign issuer on every host (T-7133).
pub fn of_token(state: &AppState, token: &str) -> Option<Arc<OrganizationRealm>> {
    here(state).filter(|realm| issuer_of(token).as_deref() == Some(realm.issuer()))
}

/// The verifier of `realm` ([`of_token`]), else the installation's.
pub fn verifier<'a>(
    state: &'a AppState,
    realm: Option<&'a OrganizationRealm>,
) -> Option<&'a BearerVerifier> {
    match realm {
        Some(realm) => Some(&realm.bearer),
        None => state.bearer.as_deref(),
    }
}

/// The client a person's `subject` token is exchanged at (RFC 8693, T-7142): the realm that
/// issued it ([`of_token`]). A token of an Organization's realm is never sent to the
/// installation's token endpoint, nor one of the installation to an Organization's.
pub async fn exchanger(state: &AppState, subject: &str) -> Result<Arc<OidcClient>, ApiError> {
    match of_token(state, subject) {
        Some(realm) => realm.client().await,
        None => state.oidc.clone().ok_or_else(|| {
            ApiError::Unavailable(
                "this Portal has no Keycloak client to exchange your token at".into(),
            )
        }),
    }
}

/// The `iss` a JWT names, unverified: only to pick the realm whose keys then verify it.
pub fn issuer_of(token: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Payload {
        iss: String,
    }
    crate::auth::oidc::jwt_payload::<Payload>(token).map(|p| p.iss.trim_end_matches('/').to_owned())
}

/// What a person of an Organization's realm cannot hold: the installation's bootstrap group is
/// the operators', who sign in through the installation realm alone (T-7001 §7 row 11). A realm
/// an Organization's admin can fill must never mint one, as a group or as a realm role of that
/// name (`permissions::in_group` counts both).
pub fn confine(identity: &mut Identity, bootstrap_admins: &str) {
    identity.groups.retain(|group| group != bootstrap_admins);
    identity.roles.retain(|role| role != bootstrap_admins);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation() -> OidcConfig {
        OidcConfig {
            issuer: "https://idm.example.test/realms/jc".parse().expect("url"),
            client_id: "portal-api".into(),
            auth: crate::auth::client_auth::ClientAuth::Secret("s".into()),
            extra_ca_pem: None,
            admin_url: None,
        }
    }

    fn hosts() -> Vec<OrganizationHost> {
        crate::organization_host::parse("hel=https://portal.hel.test,ba=https://portal.ba.test")
            .expect("hosts")
    }

    fn row(org: &str, issuer: &str, keys: &str) -> String {
        format!(r#"{{"organization":"{org}","issuer":"{issuer}","jwks_url":"{keys}"}}"#)
    }

    #[test]
    fn organization_realms_are_checked() {
        let hel = row(
            "hel",
            "https://idm.hel.test/realms/hel",
            "http://idm.hel.test/realms/hel/protocol/openid-connect/certs",
        );
        let ok = parse(&format!("[{hel}]"), Some(&installation()), &hosts()).expect("valid");
        assert_eq!(ok[0].organization, "hel");
        assert!(parse("", None, &[]).expect("empty").is_empty());
        let refused = |raw: String, why: &str| {
            let err = parse(&raw, Some(&installation()), &hosts()).expect_err(why);
            assert!(err.contains(why), "{why}: {err}");
        };
        refused(format!("[{hel},{hel}]"), "named twice");
        refused(
            format!(
                "[{hel},{}]",
                row(
                    "ba",
                    "https://idm.hel.test/realms/hel",
                    "http://idm.hel.test/realms/hel/protocol/openid-connect/certs"
                )
            ),
            "another Organization's",
        );
        refused(
            format!(
                "[{}]",
                row(
                    "hel",
                    "https://idm.example.test/realms/jc",
                    "http://idm.example.test/realms/jc/protocol/openid-connect/certs"
                )
            ),
            "installation's own",
        );
        refused(
            format!(
                "[{}]",
                row(
                    "zilina",
                    "https://idm.za.test/realms/zilina",
                    "http://idm.za.test/realms/zilina/protocol/openid-connect/certs"
                )
            ),
            "no Portal host",
        );
        refused(
            format!(
                "[{}]",
                row(
                    "hel",
                    "https://idm.hel.test/realms/hel",
                    "http://idm.hel.test/realms/ba/protocol/openid-connect/certs"
                )
            ),
            "key URL's path",
        );
        refused(
            format!(
                "[{}]",
                row(
                    "hel",
                    "https://user:pw@idm.hel.test/realms/hel",
                    "http://idm.hel.test/realms/hel/protocol/openid-connect/certs"
                )
            ),
            "without credentials",
        );
        let err = parse(&format!("[{hel}]"), None, &hosts()).expect_err("no installation realm");
        assert!(err.contains("JC_OIDC_ISSUER"), "{err}");
    }

    /// S1-04: the Portal signs in to each Organization realm with that realm's own secret, the
    /// one jc's kaisatsu derives (`jc_core::Secret::for_realm`), never the installation's.
    #[test]
    fn each_realm_signs_in_with_its_own_secret() {
        use crate::auth::client_auth::ClientAuth;
        let secret = |auth: &ClientAuth| match auth {
            ClientAuth::Secret(s) => Some(s.clone()),
            _ => None,
        };
        let admin = (
            "portal-reconciler".to_owned(),
            ClientAuth::Secret("r".into()),
        );
        let realm = |org: &str| {
            let raw = format!(
                "[{}]",
                row(
                    org,
                    &format!("https://idm.{org}.test/realms/{org}"),
                    &format!("http://idm.{org}.test/realms/{org}/protocol/openid-connect/certs"),
                )
            );
            let setting = parse(&raw, Some(&installation()), &hosts()).expect("valid");
            let base: Url = format!("https://portal.{org}.test").parse().expect("url");
            OrganizationRealm::new(&setting[0], &installation(), &base, Some(&admin))
        };
        let (hel, ba) = (realm("hel"), realm("ba"));
        let derived = jc_core::Secret::new("s").for_realm("hel", "portal-api");
        assert_eq!(secret(&hel.config.auth).as_deref(), Some(derived.expose()));
        assert_ne!(secret(&hel.config.auth), secret(&ba.config.auth));
        assert_ne!(
            secret(&hel.config.auth).as_deref(),
            Some("s"),
            "the installation's secret"
        );
        // The reconciler's syncers sign in to the realm with its own admin secret (T-7288).
        let (id, auth) = hel.admin_auth(&admin);
        assert_eq!(id, "portal-reconciler");
        assert_eq!(
            secret(&auth).as_deref(),
            Some(
                jc_core::Secret::new("r")
                    .for_realm("hel", "portal-reconciler")
                    .expose()
            )
        );
        assert_ne!(secret(&auth), secret(&ba.admin_auth(&admin).1));
        // A federated assertion names no secret and stays as it is.
        let path = std::path::PathBuf::from("/var/run/secrets/token");
        let federated = own_auth(&ClientAuth::Assertion(path.clone()), "hel", "portal-api");
        assert!(matches!(&federated, ClientAuth::Assertion(p) if *p == path));
        assert_eq!(secret(&federated), None);
    }

    /// The realm of `org` with its issuer at `idm` (`https://idm.hel.test`), as `parse` takes it.
    fn realm_at(org: &str, idm: &str) -> OrganizationRealm {
        let raw = format!(
            "[{}]",
            row(
                org,
                &format!("{idm}/realms/{org}"),
                &format!("{idm}/realms/{org}/protocol/openid-connect/certs"),
            )
        );
        let setting = parse(&raw, Some(&installation()), &hosts()).expect("valid");
        let base: Url = format!("https://portal.{org}.test").parse().expect("url");
        OrganizationRealm::new(&setting[0], &installation(), &base, None)
    }

    fn set_of(realms: Vec<OrganizationRealm>) -> OrganizationRealms {
        OrganizationRealms::new(
            realms
                .into_iter()
                .map(|r| (r.organization.clone(), Arc::new(r)))
                .collect(),
        )
    }

    /// A realm's sign-in configuration: its public issuer, the installation's client, its own
    /// secret and the back channel's origin as the admin URL; no admin client, no people.
    #[test]
    fn a_realm_signs_in_at_its_issuer_and_posts_to_its_back_channel() {
        let hel = realm_at("hel", "https://idm.hel.test");
        let oidc = hel.oidc();
        assert_eq!(oidc.issuer.as_str(), "https://idm.hel.test/realms/hel");
        assert_eq!(hel.issuer(), "https://idm.hel.test/realms/hel");
        assert_eq!(oidc.client_id, "portal-api");
        assert_eq!(
            oidc.admin_url.as_ref().map(Url::as_str),
            Some("https://idm.hel.test/")
        );
        assert_eq!(hel.backchannel, "https://idm.hel.test/realms/hel");
        assert!(hel.people.is_none());
    }

    /// The set keeps what `retain` names and nothing else, and a lock a panicking writer
    /// poisoned is still read and replaced: a crashed request never takes the realms away.
    #[test]
    fn the_set_is_narrowed_by_retain_and_survives_a_poisoned_lock() {
        let realms = set_of(vec![
            realm_at("hel", "https://idm.hel.test"),
            realm_at("ba", "https://idm.ba.test"),
        ]);
        assert_eq!(realms.keys(), ["ba", "hel"]);
        assert_eq!(realms.values().len(), 2);
        realms.retain(&["hel".to_owned(), "zz".to_owned()]);
        assert_eq!(realms.keys(), ["hel"]);
        assert!(realms.get("ba").is_none());
        assert_eq!(
            realms.get("hel").map(|r| r.organization.clone()).as_deref(),
            Some("hel")
        );

        let held = realms.clone();
        let poisoned = std::thread::spawn(move || {
            let _guard = held.0.write().expect("first writer");
            panic!("a writer dies holding the lock");
        })
        .join();
        assert!(poisoned.is_err() && realms.0.is_poisoned());
        assert_eq!(realms.keys(), ["hel"]);
        realms.retain(&[]);
        assert!(realms.keys().is_empty());
        assert!(realms.values().is_empty());
    }

    /// A realm that does not answer discovery costs that sign-in a 503 naming the Organization;
    /// one that answers is discovered once and its client kept.
    #[tokio::test]
    async fn a_realm_is_discovered_on_first_use_and_a_silent_one_is_unavailable() {
        // Port 9 refuses at once.
        let silent = realm_at("hel", "http://127.0.0.1:9");
        let refused = silent.client().await.map(|_| ());
        assert!(
            matches!(&refused, Err(ApiError::Unavailable(why)) if why.contains("hel")),
            "{refused:?}"
        );
        let server = crate::auth::oidc::tests::mock_realm().await;
        let raw = format!(
            "[{}]",
            row(
                "ba",
                &format!("{}/realms/bb", server.uri()),
                &format!("{}/realms/bb/protocol/openid-connect/certs", server.uri()),
            )
        );
        let setting = parse(&raw, Some(&installation()), &hosts()).expect("valid");
        let base: Url = "https://portal.ba.test".parse().expect("url");
        let ba = OrganizationRealm::new(&setting[0], &installation(), &base, None);
        let first = ba.client().await.expect("discovered");
        let again = ba.client().await.expect("kept");
        assert!(Arc::ptr_eq(&first, &again));
    }

    /// T-7165: an App signs its people in at its project's Organization's realm, read off the
    /// mirror; an App of a project with no Organization realm, or no App at all, uses the
    /// installation's. Each realm's token is refused on the other's App (T-7133).
    #[tokio::test]
    async fn an_apps_token_is_verified_by_its_organizations_realm_alone() {
        use crate::auth::bearer::tests::{keypair, now, sign, ISSUER};
        use crate::resource::{ObjectMeta, ResourceEnvelope};
        let envelope =
            |kind: &str, name: &str, namespace: &str, spec: serde_json::Value| ResourceEnvelope {
                api_version: crate::resource::API_VERSION.into(),
                kind: kind.into(),
                metadata: ObjectMeta {
                    name: name.into(),
                    namespace: Some(namespace.into()),
                    ..Default::default()
                },
                spec,
                status: None,
            };
        let mut state = AppState::new(crate::config::Config::for_tests(), None);
        let org = crate::permissions::ORG_NAMESPACE;
        state.mirror.upsert(envelope(
            "Project",
            "air",
            org,
            serde_json::json!({ "organizationRef": { "name": "hel" } }),
        ));
        state
            .mirror
            .upsert(envelope("Project", "free", org, serde_json::json!({})));
        state
            .mirror
            .upsert(envelope("App", "dash", "air", serde_json::json!({})));
        state
            .mirror
            .upsert(envelope("App", "plain", "free", serde_json::json!({})));

        let (installation_key, installation_set) = keypair("inst");
        let installation_verifier = BearerVerifier::with_jwks_url(
            &ISSUER.parse().expect("issuer"),
            "portal-api",
            "http://127.0.0.1:9/certs".parse().expect("jwks"),
        );
        installation_verifier.install(&installation_set);
        state.bearer = Some(Arc::new(installation_verifier));
        let hel = realm_at("hel", "https://idm.hel.test");
        let (hel_key, hel_set) = keypair("hel");
        hel.bearer.install(&hel_set);
        state.organization_realms = set_of(vec![hel]);

        assert_eq!(
            of_app(&state, "dash")
                .map(|r| r.organization.clone())
                .as_deref(),
            Some("hel")
        );
        assert!(of_app(&state, "plain").is_none());
        assert!(of_app(&state, "ghost").is_none());
        assert_eq!(
            app_oidc(&state, "dash")
                .map(|c| c.issuer.to_string())
                .as_deref(),
            Some("https://idm.hel.test/realms/hel")
        );
        assert_eq!(
            app_oidc(&state, "plain").map(|c| c.issuer),
            state.config.oidc.as_ref().map(|c| c.issuer.clone())
        );

        let token = |key: &jsonwebtoken::EncodingKey, kid: &str, iss: &str, app: &str| {
            let client = crate::reconciler::app_clients::client_id(app);
            sign(
                key,
                kid,
                &serde_json::json!({
                    "iss": iss, "typ": "Bearer", "aud": client, "azp": client, "sub": "pia",
                    "exp": now() + 60, "iat": now(),
                    "resource_access": { client.clone(): { "roles": ["editor"] } },
                }),
            )
        };
        let hel_issuer = "https://idm.hel.test/realms/hel";
        let (_, roles) =
            verify_app_token(&state, "dash", &token(&hel_key, "hel", hel_issuer, "dash"))
                .await
                .expect("the App's realm");
        assert_eq!(roles, ["editor"]);
        let (_, roles) = verify_app_token(
            &state,
            "plain",
            &token(&installation_key, "inst", ISSUER, "plain"),
        )
        .await
        .expect("the installation's realm");
        assert_eq!(roles, ["editor"]);
        for (app, signed) in [
            ("dash", token(&installation_key, "inst", ISSUER, "dash")),
            ("plain", token(&hel_key, "hel", hel_issuer, "plain")),
            ("dash", token(&hel_key, "hel", hel_issuer, "plain")),
        ] {
            assert!(
                matches!(
                    verify_app_token(&state, app, &signed).await,
                    Err(ApiError::Unauthorized)
                ),
                "{app}"
            );
        }
        state.bearer = None;
        assert!(matches!(
            verify_app_token(
                &state,
                "plain",
                &token(&installation_key, "inst", ISSUER, "plain")
            )
            .await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// T-7133, T-7142: on an Organization's host its realm checks and exchanges the tokens it
    /// issued; the installation's tokens, and any token off that host, go to the installation.
    #[tokio::test]
    async fn a_token_is_checked_and_exchanged_by_the_realm_that_issued_it() {
        use crate::organization_host::scope;
        use base64::Engine as _;
        let jwt = |iss: &str| {
            let b64 = |v: serde_json::Value| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
            };
            format!(
                "{}.{}.sig",
                b64(serde_json::json!({ "alg": "ES256" })),
                b64(serde_json::json!({ "iss": iss }))
            )
        };
        let mut state = AppState::new(crate::config::Config::for_tests(), None);
        state.organization_realms = set_of(vec![realm_at("hel", "http://127.0.0.1:9")]);
        let hel_token = jwt("http://127.0.0.1:9/realms/hel/");
        let installation_token = jwt("https://idm.example.test/realms/jc");
        assert_eq!(
            issuer_of(&hel_token).as_deref(),
            Some("http://127.0.0.1:9/realms/hel")
        );
        assert_eq!(issuer_of("not-a-jwt"), None);
        let host = hosts()
            .into_iter()
            .find(|h| h.organization == "hel")
            .expect("hel");

        let (on_hel, foreign, here_org) = scope(host.clone(), {
            let state = state.clone();
            let (hel_token, installation_token) = (hel_token.clone(), installation_token.clone());
            async move {
                (
                    of_token(&state, &hel_token).map(|r| r.organization.clone()),
                    of_token(&state, &installation_token).is_some(),
                    here(&state).map(|r| r.organization.clone()),
                )
            }
        })
        .await;
        assert_eq!(on_hel.as_deref(), Some("hel"));
        assert!(!foreign);
        assert_eq!(here_org.as_deref(), Some("hel"));
        assert!(
            of_token(&state, &hel_token).is_none(),
            "off the Organization's host"
        );
        assert!(here(&state).is_none());

        let realm = state.organization_realms.get("hel").expect("hel");
        assert!(std::ptr::eq(
            verifier(&state, Some(&realm)).expect("the realm's"),
            &realm.bearer
        ));
        assert!(
            verifier(&state, None).is_none(),
            "a Portal with no realm verifies nothing"
        );

        // The realm that issued it is asked, and it does not answer: a 503 naming it, never the
        // installation's client.
        let exchanged = scope(host, {
            let state = state.clone();
            async move { exchanger(&state, &hel_token).await.map(|_| ()) }
        })
        .await;
        assert!(
            matches!(&exchanged, Err(ApiError::Unavailable(why)) if why.contains("hel")),
            "{exchanged:?}"
        );
        let refused = exchanger(&state, &installation_token).await.map(|_| ());
        assert!(
            matches!(&refused, Err(ApiError::Unavailable(why)) if why.contains("no Keycloak client")),
            "{refused:?}"
        );
    }

    #[test]
    fn a_realm_never_mints_the_operators_group() {
        let mut identity = Identity {
            subject: "s".into(),
            username: "u".into(),
            email: None,
            name: None,
            roles: vec!["portal-approver".into(), "hel-reader".into()],
            groups: vec!["portal-approver".into(), "hel-editors".into()],
            client: None,
            second_factor: false,
        };
        confine(&mut identity, "portal-approver");
        assert_eq!(identity.groups, vec!["hel-editors".to_owned()]);
        // A realm role of that name counts as the group in every bootstrap-admin check.
        assert_eq!(identity.roles, vec!["hel-reader".to_owned()]);
        assert!(!crate::permissions::in_group(&identity, "portal-approver"));
    }
}
