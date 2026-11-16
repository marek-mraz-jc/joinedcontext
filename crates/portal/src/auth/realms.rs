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

use std::sync::Arc;

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
    pub people: Option<crate::people::People>,
    /// The realm's issuer path at its back channel, where the Portal's admin clients post
    /// (`http://idm.city.example/realms/city`).
    pub backchannel: String,
}

impl OrganizationRealm {
    /// The realm of `setting`, signing in as the installation's Portal client (T-7001 §3: one
    /// client secret for every realm) with the redirect URI of the Organization's host `base`,
    /// and managing its people as the installation's admin client `admin` of the same name.
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
                crate::people::People::new(&backchannel, id.clone(), auth.clone())
            }),
            backchannel,
            organization: setting.organization.clone(),
            issuer: trimmed(&setting.issuer).to_owned(),
            config: OidcConfig {
                issuer: setting.issuer.clone(),
                client_id: installation.client_id.clone(),
                auth: installation.auth.clone(),
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
    state.organization_realms.get(&organization).cloned()
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
/// an Organization's admin can fill must never mint one.
pub fn confine(identity: &mut Identity, bootstrap_admins: &str) {
    identity.groups.retain(|group| group != bootstrap_admins);
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

    #[test]
    fn a_realm_never_mints_the_operators_group() {
        let mut identity = Identity {
            subject: "s".into(),
            username: "u".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec!["portal-approver".into(), "hel-editors".into()],
            client: None,
            second_factor: false,
        };
        confine(&mut identity, "portal-approver");
        assert_eq!(identity.groups, vec!["hel-editors".to_owned()]);
    }
}
