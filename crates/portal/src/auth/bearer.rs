//! Bearer-token verification for service callers and for the edge's user token (docs
//! Architecture/12 §3, OPS-33, ADR-N-019).
//!
//! The APISIX edge forwards `Authorization` untouched and the Portal is the verifier. Tokens are
//! checked against the realm JWKS, cached in memory; an unknown `kid` triggers at most one
//! refetch per minute, so a flood of forged tokens cannot turn the Portal into a Keycloak load
//! generator.
//!
//! Two signature algorithms and no third: the realm signs ES256 (TR-03187 AR-11), and the
//! `edge` client the `openid-connect` plugin logs people in with is signed RS256 by per-client
//! override, because lua-resty-openidc verifies RS and HS only. The user's access token reaches
//! the Portal as `X-Access-Token` and is verified here like any bearer (AP-27, AP-28).

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use url::Url;

use crate::auth::session::{proves_second_factor, Identity, Session};
use crate::error::ApiError;

/// The signature algorithms the realm uses: ES256 for every client (TR-03187 AR-11) and RS256
/// for the `edge` client alone (ADR-N-019). HS256 is never one of them: a shared secret would
/// let anyone who holds it mint a token.
const ALGORITHMS: [Algorithm; 2] = [Algorithm::ES256, Algorithm::RS256];

/// The `typ` of a Keycloak access token.
const ACCESS: &str = "Bearer";
/// The `typ` of a Keycloak back-channel logout token.
const LOGOUT: &str = "Logout";

/// Whether a JWK's `alg` (or, absent one, its key type) is one of [`ALGORITHMS`].
fn usable(jwk: &jsonwebtoken::jwk::Jwk) -> bool {
    match jwk.common.key_algorithm {
        Some(alg) => matches!(alg.to_string().as_str(), "ES256" | "RS256"),
        None => matches!(
            jwk.algorithm,
            jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(_)
                | jsonwebtoken::jwk::AlgorithmParameters::RSA(_)
        ),
    }
}
/// Minimum distance between two JWKS fetches caused by unknown key ids.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub struct BearerVerifier {
    issuer: String,
    audience: String,
    jwks_url: Url,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
    last_refresh: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    /// Keycloak's token type: `Bearer` for an access token, `ID` for an ID token, `Logout` for a
    /// back-channel logout token. Each door takes one type alone (F-R2POR3-010).
    typ: Option<String>,
    exp: i64,
    #[serde(default)]
    iat: i64,
    /// When the person signed in to the SSO session: the same on every refreshed token, so it
    /// names the session where `iat` names one token (JC2-19).
    auth_time: Option<i64>,
    preferred_username: Option<String>,
    azp: Option<String>,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    #[serde(default)]
    realm_access: RealmAccess,
    #[serde(default)]
    groups: Vec<String>,
    /// Client roles by client id, as Keycloak's `roles` scope writes them.
    #[serde(default)]
    resource_access: HashMap<String, RealmAccess>,
    /// Authentication context and methods, which say whether a second factor was given (T-5567).
    acr: Option<String>,
    #[serde(default)]
    amr: serde_json::Value,
    /// The granted OAuth scopes, space separated.
    #[serde(default)]
    scope: String,
}

#[derive(Default, Deserialize)]
struct RealmAccess {
    #[serde(default)]
    roles: Vec<String>,
}

impl BearerVerifier {
    /// `issuer` is the realm URL (`…/realms/{realm}`); the JWKS lives at its `certs` endpoint.
    pub fn new(issuer: &Url, audience: &str) -> Self {
        let mut jwks_url = issuer.clone();
        let path = format!(
            "{}/protocol/openid-connect/certs",
            issuer.path().trim_end_matches('/')
        );
        jwks_url.set_path(&path);
        Self::with_jwks_url(issuer, audience, jwks_url)
    }

    pub fn with_jwks_url(issuer: &Url, audience: &str, jwks_url: Url) -> Self {
        Self {
            issuer: issuer.as_str().trim_end_matches('/').to_string(),
            audience: audience.to_string(),
            jwks_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("static reqwest client configuration"),
            keys: RwLock::new(HashMap::new()),
            last_refresh: Mutex::new(None),
        }
    }

    /// Replaces the cached keys with the ES256 and RS256 keys of `set`; returns how many were
    /// usable.
    pub fn install(&self, set: &JwkSet) -> usize {
        let keys: HashMap<String, DecodingKey> = set
            .keys
            .iter()
            .filter(|jwk| usable(jwk))
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone()?;
                DecodingKey::from_jwk(jwk).ok().map(|key| (kid, key))
            })
            .collect();
        let n = keys.len();
        if let Ok(mut cache) = self.keys.write() {
            *cache = keys;
        }
        n
    }

    /// Fetches the JWKS, at most once a minute; inside the interval it is a
    /// no-op that reports the current key count. A failed fetch also starts the interval.
    pub async fn refresh(&self) -> Result<usize, ApiError> {
        {
            let mut last = self
                .last_refresh
                .lock()
                .map_err(|_| ApiError::Internal("jwks lock".into()))?;
            if last.is_some_and(|t| t.elapsed() < REFRESH_INTERVAL) {
                return Ok(self.keys.read().map(|k| k.len()).unwrap_or(0));
            }
            *last = Some(Instant::now());
        }
        let set: JwkSet = self
            .http
            .get(self.jwks_url.clone())
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| ApiError::Unavailable(format!("JWKS fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Unavailable(format!("JWKS is not a key set: {e}")))?;
        Ok(self.install(&set))
    }

    fn key(&self, kid: &str) -> Option<DecodingKey> {
        self.keys.read().ok()?.get(kid).cloned()
    }

    /// Verifies signature (ES256 or RS256, known `kid`), `iss`, `aud`, `exp` and `nbf`; anything
    /// else is `401`. The returned session carries no id token: bearer callers never log out.
    pub async fn verify(&self, token: &str) -> Result<Session, ApiError> {
        self.verify_with_client(token)
            .await
            .map(|(session, _)| session)
    }

    /// [`Self::verify`], and the Keycloak client that obtained the token (`azp`): a route that
    /// belongs to one workload asks which client it is, because a user name is not that.
    pub async fn verify_with_client(
        &self,
        token: &str,
    ) -> Result<(Session, Option<String>), ApiError> {
        self.verify_for_audience(token, &self.audience).await
    }

    /// [`Self::verify_with_client`] against an audience other than this verifier's own: the
    /// internal listener takes tokens bound to `portal-internal` rather than to the Portal's API
    /// client, and one verifier with one key cache serves both (PF-46, AG-52). The issuer, the
    /// algorithms and the required claims are the same; only what the token must be *for* differs.
    pub async fn verify_for_audience(
        &self,
        token: &str,
        audience: &str,
    ) -> Result<(Session, Option<String>), ApiError> {
        let claims = self.claims(token, audience).await?;
        // A connector token also names the Portal's client, which exchanges it for Endpoint
        // reads; it is taken at its connector's door alone (T-5954, AG-151).
        if has_family(&claims.scope, crate::connectors::SCOPE_FAMILY) {
            return Err(ApiError::Unauthorized);
        }
        Ok(session_of(claims))
    }

    /// [`Self::verify_for_audience`], and the token's scopes of `scope`'s family (the text up to
    /// its `:`) are exactly `scope`: a token naming two connectors is a token for neither
    /// (T-5954).
    pub async fn verify_for_scope(
        &self,
        token: &str,
        audience: &str,
        scope: &str,
    ) -> Result<(Session, Option<String>), ApiError> {
        let claims = self.claims(token, audience).await?;
        if !names_only(&claims.scope, scope) {
            return Err(ApiError::Unauthorized);
        }
        Ok(session_of(claims))
    }

    /// A person's token of an App's own Keycloak client (ADR-N-030, AP-92): issued for
    /// `client` and obtained by it (`azp`), and the roles `resource_access.{client}.roles`
    /// gives. A role of any other client, a realm role and a group are not the App's roles, and
    /// a token another client obtained is refused whatever audience it carries.
    pub async fn verify_app_token(
        &self,
        token: &str,
        client: &str,
    ) -> Result<(Session, Vec<String>), ApiError> {
        let mut claims = self.claims(token, client).await?;
        if claims.azp.as_deref() != Some(client) {
            return Err(ApiError::Unauthorized);
        }
        let roles = claims
            .resource_access
            .remove(client)
            .map(|access| access.roles)
            .unwrap_or_default();
        Ok((session_of(claims).0, roles))
    }

    /// A logout token the realm sends to `audience` back-channel: verified as an access token
    /// is, and of type `Logout`, which no access or ID token carries (OIDC Back-Channel Logout
    /// 1.0 section 2.4).
    pub async fn verify_logout_token(
        &self,
        token: &str,
        audience: &str,
    ) -> Result<String, ApiError> {
        self.claims_of_type(token, audience, LOGOUT)
            .await
            .map(|claims| claims.sub)
    }

    /// An access token's claims. An ID token is refused although the realm signs it with the
    /// same key and for the Portal's own client: it travels in the open as `id_token_hint`, so
    /// it is no credential (F-R2POR3-010).
    async fn claims(&self, token: &str, audience: &str) -> Result<Claims, ApiError> {
        self.claims_of_type(token, audience, ACCESS).await
    }

    async fn claims_of_type(
        &self,
        token: &str,
        audience: &str,
        typ: &str,
    ) -> Result<Claims, ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::Unauthorized)?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(ApiError::Unauthorized);
        }
        let kid = header.kid.ok_or(ApiError::Unauthorized)?;
        let key = match self.key(&kid) {
            Some(key) => key,
            None => {
                // A rotated realm key is the only legitimate reason for an unknown kid.
                if let Err(err) = self.refresh().await {
                    tracing::warn!(error = %err, "JWKS refresh after unknown kid failed");
                }
                self.key(&kid).ok_or(ApiError::Unauthorized)?
            }
        };
        // The token's own algorithm, which the header check above narrowed to the two allowed
        // ones; `decode` refuses a key of the other family, so an RS256 header cannot be
        // verified against an EC key or the other way round.
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        decode::<Claims>(token, &key, &validation)
            .map(|data| data.claims)
            .ok()
            .filter(|claims| claims.typ.as_deref() == Some(typ))
            .ok_or(ApiError::Unauthorized)
    }
}

/// The session a verified token stands for, and the client that obtained it.
/// The scopes of `granted` in `family` (`family:...`).
fn of_family<'a>(granted: &'a str, family: &'a str) -> impl Iterator<Item = &'a str> {
    granted
        .split_whitespace()
        .filter(move |s| s.split_once(':').is_some_and(|(f, _)| f == family))
}

fn has_family(granted: &str, family: &str) -> bool {
    of_family(granted, family).next().is_some()
}

/// Whether `granted` holds `scope` and no other scope of its family.
fn names_only(granted: &str, scope: &str) -> bool {
    let family = scope.split_once(':').map_or(scope, |(family, _)| family);
    let mut named = of_family(granted, family);
    named.next() == Some(scope) && named.next().is_none()
}

fn session_of(c: Claims) -> (Session, Option<String>) {
    let client = c.azp.clone();
    let username = c
        .preferred_username
        .or(c.azp)
        .unwrap_or_else(|| c.sub.clone());
    let second_factor = proves_second_factor(c.acr.as_deref(), &c.amr);
    let session = Session {
        identity: Identity {
            client: client.clone(),
            second_factor,
            subject: c.sub,
            username,
            email: super::session::verified_email(c.email, c.email_verified),
            name: c.name,
            roles: c.realm_access.roles,
            groups: c
                .groups
                .into_iter()
                .map(|g| g.trim_start_matches('/').to_owned())
                .collect(),
        },
        expires_at: c.exp,
        issued_at: c.iat,
        signed_in_at: c.auth_time,
        id_token: String::new(),
        access_expires_at: c.exp,
        refresh_token: None,
    };
    (session, client)
}

/// The Keycloak client (`azp`) of a verified bearer token, parked in the request's extensions
/// by [`crate::auth::session::CurrentUser`]. Absent for a cookie session and for the edge's
/// token, which is a person's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenClient(pub String);

#[cfg(test)]
mod tests {
    /// ADR-N-067 2.2 (T-6408): the `groups` claim holds full paths; the session names a root
    /// group by its name and keeps an Organization's subgroup as `helsinki/members`, which no
    /// RoleBinding subject, project or Group name can equal.
    #[test]
    fn a_full_path_group_loses_its_leading_slash_and_a_subgroup_keeps_its_parent() {
        let claims = serde_json::json!({
            "sub": "ana", "exp": 400, "iat": 300,
            "groups": ["/helsinki", "/helsinki/members"],
        });
        let (session, _) =
            super::session_of(serde_json::from_value::<super::Claims>(claims).expect("claims"));
        assert_eq!(session.identity.groups, ["helsinki", "helsinki/members"]);
    }

    /// JC2-19: a bearer session carries its sign-in (`auth_time`), the same on every refreshed
    /// token, beside the token's own `iat`.
    #[test]
    fn a_refreshed_token_keeps_its_sessions_start() {
        let claims = |extra: serde_json::Value| {
            let mut c = serde_json::json!({ "sub": "ana", "exp": 400, "iat": 300 });
            if let (Some(c), Some(extra)) = (c.as_object_mut(), extra.as_object()) {
                c.extend(extra.clone());
            }
            serde_json::from_value::<super::Claims>(c).expect("claims")
        };
        let (session, _) = super::session_of(claims(serde_json::json!({ "auth_time": 100 })));
        // Revocation keeps reading the token's own `iat` (a refreshed token after a logout
        // mark answers again); the paired tab reads the sign-in.
        assert_eq!(
            (session.issued_at, session.signed_in_at, session.expires_at),
            (300, Some(100), 400)
        );
        let (session, _) = super::session_of(claims(serde_json::json!({})));
        assert_eq!((session.issued_at, session.signed_in_at), (300, None));
    }

    #[test]
    fn a_scoped_token_names_its_one_connector_and_no_other() {
        let a = "connector:abcdefghijklmnopqrstuvwxyz";
        assert!(super::names_only(&format!("openid {a} profile"), a));
        assert!(!super::names_only("openid profile", a));
        assert!(!super::names_only(
            "openid connector:bcdefghijklmnopqrstuvwxyz2",
            a
        ));
        assert!(!super::names_only(
            &format!("{a} connector:bcdefghijklmnopqrstuvwxyz2"),
            a
        ));
        assert!(!super::names_only(&format!("{a}x"), a));
        assert!(super::has_family(&format!("openid {a}"), "connector"));
        assert!(!super::has_family(
            "openid connectors endpoint:x",
            "connector"
        ));
    }

    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use p256::pkcs8::EncodePrivateKey;
    use serde_json::json;

    const ISSUER: &str = "https://idm.example.test/realms/dev";

    fn verifier() -> BearerVerifier {
        // Port 9 (discard) refuses connections immediately: a refresh fails fast, never hangs.
        BearerVerifier::with_jwks_url(
            &Url::parse(ISSUER).unwrap(),
            "portal-api",
            Url::parse("http://127.0.0.1:9/certs").unwrap(),
        )
    }

    /// A fresh P-256 key pair: the signing key for `encode` and its public half as a JWK set.
    fn keypair(kid: &str) -> (EncodingKey, JwkSet) {
        let secret = p256::SecretKey::random(&mut rand_core::OsRng);
        let der = secret.to_pkcs8_der().unwrap();
        let signer = EncodingKey::from_ec_der(der.as_bytes());
        let mut jwk: serde_json::Value =
            serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(kid);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");
        let set: JwkSet = serde_json::from_value(json!({ "keys": [jwk] })).unwrap();
        (signer, set)
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn claims(exp: i64) -> serde_json::Value {
        json!({
            "iss": ISSUER, "typ": "Bearer", "aud": "portal-api", "sub": "svc:smoke", "azp": "apisix-gateway",
            "exp": exp, "iat": now(), "realm_access": { "roles": ["viewer"] }
        })
    }

    /// An unverified address is no address: a binding naming it would otherwise go to whoever
    /// typed it into their account.
    #[test]
    fn only_a_verified_email_reaches_the_identity() {
        let identity = |verified: serde_json::Value| {
            let mut c = claims(now() + 60);
            c["email"] = json!("admin@city.example");
            c["email_verified"] = verified;
            let c: Claims = serde_json::from_value(c).unwrap();
            session_of(c).0.identity.email
        };
        assert_eq!(identity(json!(false)), None);
        assert_eq!(identity(serde_json::Value::Null), None);
        assert_eq!(identity(json!(true)).as_deref(), Some("admin@city.example"));
    }

    fn sign(signer: &EncodingKey, kid: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.into());
        encode(&header, claims, signer).unwrap()
    }

    // ---------------------------------------------------------------------------------------
    // T-2091 `refresh`: the interval, and what a failure says and to whom.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_failed_fetch_starts_the_interval_so_the_next_call_does_not_hammer_the_realm() {
        let v = verifier(); // port 9: the connection is refused at once
        let (_signer, set) = keypair("k1");
        assert_eq!(v.install(&set), 1);

        // The first call tries and fails; it must not throw the installed keys away.
        let first = v.refresh().await;
        assert!(matches!(first, Err(ApiError::Unavailable(_))), "{first:?}");
        assert!(v.key("k1").is_some(), "a failed refresh emptied the cache");

        // Inside the interval the next call is a no-op that reports what is cached, however many
        // times it is called: a route that verifies a token with an unknown kid cannot turn one
        // request into one realm fetch each (REFRESH_INTERVAL).
        for _ in 0..5 {
            assert!(matches!(v.refresh().await, Ok(1)));
        }
    }

    #[tokio::test]
    async fn a_refresh_failure_reaches_the_log_and_never_a_caller() {
        // The message carries the reqwest error, which names the JWKS URL — an internal realm
        // address. That is a log line, never a body: both callers swallow it
        // (`src/state.rs:207` and `verify_for_audience` at bearer.rs:187, each `if let Err(err)`),
        // and the verification itself answers a bare 401. Asserted here so a caller that starts
        // propagating it is noticed.
        let v = verifier();
        let Err(ApiError::Unavailable(message)) = v.refresh().await else {
            panic!("a refused connection should be Unavailable");
        };
        assert!(message.starts_with("JWKS fetch failed"));

        // The door a caller actually knocks on: an unknown kid with the realm unreachable is 401
        // with nothing about the realm in it.
        let (signer, _set) = keypair("k1");
        let refused = v.verify(&sign(&signer, "k1", &claims(now() + 60))).await;
        assert!(
            matches!(refused, Err(ApiError::Unauthorized)),
            "{refused:?}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // T-2092 `verify_with_client`: what it accepts, what it refuses, and what it hands back.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_client_comes_back_with_the_session_and_is_absent_when_the_token_has_no_azp() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        let (session, client) = v
            .verify_with_client(&sign(&signer, "k1", &claims(now() + 60)))
            .await
            .unwrap();
        assert_eq!(client.as_deref(), Some("apisix-gateway"));
        assert_eq!(session.identity.username, "apisix-gateway");

        // No `azp`: no client, and the name falls back to the subject rather than to nothing.
        let mut anonymous_client = claims(now() + 60);
        anonymous_client["azp"] = json!(null);
        let (session, client) = v
            .verify_with_client(&sign(&signer, "k1", &anonymous_client))
            .await
            .unwrap();
        assert_eq!(client, None);
        assert_eq!(session.identity.username, "svc:smoke");

        // A person's token: the username is theirs, and the client is still named.
        let mut human = claims(now() + 60);
        human["preferred_username"] = json!("jana.kovacova");
        let (session, client) = v
            .verify_with_client(&sign(&signer, "k1", &human))
            .await
            .unwrap();
        assert_eq!(session.identity.username, "jana.kovacova");
        assert_eq!(client.as_deref(), Some("apisix-gateway"));
    }

    #[tokio::test]
    async fn a_token_that_is_not_one_is_refused_without_a_key_lookup() {
        let v = verifier();
        let (_signer, set) = keypair("k1");
        v.install(&set);
        for token in [
            "",
            "   ",
            "not.a.token",
            "onlyonesegment",
            "two.segments",
            "a.b.c.d",
            // A header that is valid base64 JSON but names no kid.
            &jsonwebtoken::encode(
                &Header::new(Algorithm::ES256),
                &claims(now() + 60),
                &keypair("k1").0,
            )
            .unwrap(),
        ] {
            assert!(
                matches!(
                    v.verify_with_client(token).await,
                    Err(ApiError::Unauthorized)
                ),
                "{token:?} was not refused",
            );
        }
    }

    #[tokio::test]
    async fn a_missing_required_claim_is_refused_and_the_leeway_is_thirty_seconds() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        // `exp`, `iss`, `aud` and `sub` are required: a token without one of them is 401, whatever
        // else it carries.
        for missing in ["exp", "iss", "aud", "sub"] {
            let mut without = claims(now() + 60);
            without[missing] = json!(null);
            assert!(
                matches!(
                    v.verify_with_client(&sign(&signer, "k1", &without)).await,
                    Err(ApiError::Unauthorized)
                ),
                "a token without {missing} passed",
            );
        }

        // `nbf` is validated: a token not yet valid is refused, and one inside the 30 s leeway is
        // taken, because two clocks in a cluster are never the same (`validation.leeway = 30`).
        let mut future = claims(now() + 600);
        future["nbf"] = json!(now() + 3600);
        assert!(matches!(
            v.verify_with_client(&sign(&signer, "k1", &future)).await,
            Err(ApiError::Unauthorized)
        ));
        let mut just_now = claims(now() + 600);
        just_now["nbf"] = json!(now() + 10);
        assert!(v
            .verify_with_client(&sign(&signer, "k1", &just_now))
            .await
            .is_ok());

        // An expiry a second inside the leeway is still taken; one well outside it is not.
        assert!(v
            .verify_with_client(&sign(&signer, "k1", &claims(now() - 10)))
            .await
            .is_ok());
        assert!(matches!(
            v.verify_with_client(&sign(&signer, "k1", &claims(now() - 3600)))
                .await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// AP-92, ADR-N-030: an App's roles are `resource_access.app-{name}.roles` of a token of
    /// that App's own client (audience and `azp`); another client's roles, a realm role and a
    /// token another client obtained for this audience give nothing.
    #[tokio::test]
    async fn an_app_token_gives_the_apps_client_roles_and_nothing_else() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let app_token = |azp: &str| {
            let mut c = claims(now() + 60);
            c["aud"] = json!(["app-alerts", "slug-of-its-endpoint"]);
            c["azp"] = json!(azp);
            c["sub"] = json!("5f0c");
            c["preferred_username"] = json!("jana@hel.fi");
            c["email"] = json!("jana@hel.fi");
            c["email_verified"] = json!(true);
            c["realm_access"] = json!({ "roles": ["steward", "portal-approver"] });
            c["resource_access"] = json!({
                "app-alerts": { "roles": ["viewer"] },
                "app-other": { "roles": ["steward"] },
            });
            sign(&signer, "k1", &c)
        };

        let (session, roles) = v
            .verify_app_token(&app_token("app-alerts"), "app-alerts")
            .await
            .expect("the App's own token");
        assert_eq!(roles, ["viewer"], "the App's client roles alone");
        assert_eq!(session.identity.subject, "5f0c");
        assert_eq!(session.identity.email.as_deref(), Some("jana@hel.fi"));

        // Obtained by another client, even one that put this App in the audience.
        assert!(matches!(
            v.verify_app_token(&app_token("edge"), "app-alerts").await,
            Err(ApiError::Unauthorized)
        ));
        // Issued for another App.
        assert!(matches!(
            v.verify_app_token(&app_token("app-alerts"), "app-other")
                .await,
            Err(ApiError::Unauthorized)
        ));
        // A token with no roles of this client holds none.
        let mut bare = claims(now() + 60);
        bare["aud"] = json!("app-alerts");
        bare["azp"] = json!("app-alerts");
        let (_, roles) = v
            .verify_app_token(&sign(&signer, "k1", &bare), "app-alerts")
            .await
            .expect("a valid token");
        assert!(roles.is_empty());
    }

    #[tokio::test]
    async fn an_audience_list_is_read_and_a_group_keeps_no_leading_slash() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        // Keycloak sends `aud` as a list when a token is for more than one client: ours has to be
        // in it, and a list that does not name it is refused.
        let mut many = claims(now() + 60);
        many["aud"] = json!(["context-gateway", "portal-api"]);
        assert!(v
            .verify_with_client(&sign(&signer, "k1", &many))
            .await
            .is_ok());
        let mut others = claims(now() + 60);
        others["aud"] = json!(["context-gateway", "portal-internal"]);
        assert!(matches!(
            v.verify_with_client(&sign(&signer, "k1", &others)).await,
            Err(ApiError::Unauthorized)
        ));

        // A realm group is `/editors` on the wire and `editors` in a grant, so the slash goes.
        let mut grouped = claims(now() + 60);
        grouped["groups"] = json!(["/editors", "nested/one", "/a/b"]);
        let (session, _) = v
            .verify_with_client(&sign(&signer, "k1", &grouped))
            .await
            .unwrap();
        assert_eq!(
            session.identity.groups,
            vec!["editors", "nested/one", "a/b"]
        );
    }

    #[tokio::test]
    async fn a_verified_bearer_carries_no_id_token_and_no_refresh_token() {
        // A bearer caller has nothing to log out of and nothing to refresh with; a session that
        // carried either would hand a script a credential it never presented (AG-59).
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let (session, _) = v
            .verify_with_client(&sign(&signer, "k1", &claims(now() + 60)))
            .await
            .unwrap();
        assert!(session.id_token.is_empty());
        assert_eq!(session.refresh_token, None);
        assert_eq!(session.access_expires_at, session.expires_at);
    }

    /// F-R2POR3-010: an ID token for the Portal's own client, which travels in the open as
    /// `id_token_hint`, and a logout token are no bearer credential; a logout token is taken at
    /// the logout door alone, and an access token is not one there.
    #[tokio::test]
    async fn only_an_access_token_is_a_bearer_and_only_a_logout_token_logs_out() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let typed = |typ: Option<&str>| {
            let mut c = claims(now() + 60);
            match typ {
                Some(typ) => c["typ"] = json!(typ),
                None => {
                    c.as_object_mut().map(|c| c.remove("typ"));
                }
            }
            sign(&signer, "k1", &c)
        };
        for refused in [Some("ID"), Some("Logout"), Some("Refresh"), None] {
            assert!(
                matches!(v.verify(&typed(refused)).await, Err(ApiError::Unauthorized)),
                "{refused:?} passed as a bearer"
            );
        }
        assert!(v.verify(&typed(Some("Bearer"))).await.is_ok());
        assert_eq!(
            v.verify_logout_token(&typed(Some("Logout")), "portal-api")
                .await
                .unwrap(),
            "svc:smoke"
        );
        assert!(v
            .verify_logout_token(&typed(Some("Bearer")), "portal-api")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_valid_es256_token_yields_the_service_identity() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        assert_eq!(v.install(&set), 1);
        let session = v
            .verify(&sign(&signer, "k1", &claims(now() + 60)))
            .await
            .unwrap();
        assert_eq!(session.identity.subject, "svc:smoke");
        assert_eq!(session.identity.username, "apisix-gateway");
        assert_eq!(session.identity.roles, vec!["viewer"]);
        assert!(!session.is_expired(now()));
    }

    #[tokio::test]
    async fn expired_wrong_audience_wrong_issuer_and_other_key_are_refused() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let (other, _) = keypair("k1");
        let mut foreign_aud = claims(now() + 60);
        foreign_aud["aud"] = json!("context-gateway");
        let mut foreign_iss = claims(now() + 60);
        foreign_iss["iss"] = json!("https://idm.example.test/realms/other");
        for token in [
            sign(&signer, "k1", &claims(now() - 120)),
            sign(&signer, "k1", &foreign_aud),
            sign(&signer, "k1", &foreign_iss),
            sign(&other, "k1", &claims(now() + 60)),
        ] {
            assert!(matches!(
                v.verify(&token).await,
                Err(ApiError::Unauthorized)
            ));
        }
    }

    #[tokio::test]
    async fn a_tampered_signature_and_an_unsigned_expired_token_are_refused() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);
        let good = sign(&signer, "k1", &claims(now() + 60));
        let tampered = format!("{}.forged", good.rsplit_once('.').unwrap().0);
        assert!(matches!(
            v.verify(&tampered).await,
            Err(ApiError::Unauthorized)
        ));
        // The same token scripts/smoke.sh in joinedcontext-deployment sends (built here, not a
        // literal: a JWT-shaped string in the repo trips gitleaks).
        use base64::Engine;
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let unsigned = format!(
            "{}.{}.AAAA",
            b64(r#"{"alg":"ES256","typ":"JWT"}"#),
            b64(r#"{"exp":1}"#)
        );
        assert!(matches!(
            v.verify(&unsigned).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            v.verify("not-a-jwt").await,
            Err(ApiError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn a_shared_secret_algorithm_is_refused_even_with_the_right_claims() {
        let v = verifier();
        let (_, set) = keypair("k1");
        v.install(&set);
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("k1".into());
        let hs = encode(
            &header,
            &claims(now() + 60),
            &EncodingKey::from_secret(b"shared"),
        )
        .unwrap();
        assert!(matches!(v.verify(&hs).await, Err(ApiError::Unauthorized)));
    }

    #[test]
    fn a_jwk_of_another_algorithm_is_not_installed() {
        let v = verifier();
        let (_, mut set) = keypair("k1");
        // Keycloak lists the realm's RSA keys with `alg: RS256`; an HMAC or an encryption key
        // in the same set must not become a verification key.
        let mut oct: serde_json::Value = serde_json::to_value(&set.keys[0]).unwrap();
        oct["kid"] = json!("k-enc");
        oct["alg"] = json!("RSA-OAEP");
        set.keys.push(serde_json::from_value(oct).unwrap());
        assert_eq!(v.install(&set), 1, "only the ES256 key is usable");
    }

    #[tokio::test]
    async fn an_unknown_kid_fetches_once_per_minute_and_is_refused() {
        let v = verifier();
        let (signer, _) = keypair("k9");
        let token = sign(&signer, "k9", &claims(now() + 60));
        assert!(matches!(
            v.verify(&token).await,
            Err(ApiError::Unauthorized)
        ));
        // The failed fetch started the interval: the next refresh is a no-op, not a request.
        assert_eq!(v.refresh().await.unwrap(), 0);
    }

    // ---------------------------------------------------------------------------------------
    // T-2089 `with_jwks_url`: what the verifier is bound to, and what its HTTP client will do.
    // ---------------------------------------------------------------------------------------

    /// AP-27: the JWKS URL is a realm address the Portal trusts. The client follows no redirect,
    /// so whoever can answer that address cannot send the Portal to fetch its verification keys
    /// from somewhere else — the keys would then be the redirector's and every forged token of
    /// theirs would verify.
    #[tokio::test]
    async fn the_client_follows_no_redirect_so_the_keys_cannot_be_fetched_elsewhere() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let realm = MockServer::start().await;
        let (_signer, set) = keypair("attacker");
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/elsewhere"))
            .mount(&realm)
            .await;
        Mock::given(method("GET"))
            .and(path("/elsewhere"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&set))
            .mount(&realm)
            .await;

        let v = BearerVerifier::with_jwks_url(
            &Url::parse(ISSUER).unwrap(),
            "portal-api",
            Url::parse(&format!("{}/certs", realm.uri())).unwrap(),
        );
        let refreshed = v.refresh().await;
        assert!(
            matches!(refreshed, Err(ApiError::Unavailable(_))),
            "the redirect was followed: {refreshed:?}"
        );
        assert!(
            v.key("attacker").is_none(),
            "a redirected key set was installed"
        );
    }

    /// AP-27: a realm that accepts the connection and never answers must not hold a Portal
    /// request open. The client's own five-second timeout ends it.
    #[tokio::test]
    async fn a_realm_that_never_answers_times_out_instead_of_holding_the_request() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let realm = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "keys": [] }))
                    .set_delay(Duration::from_secs(60)),
            )
            .mount(&realm)
            .await;

        let v = BearerVerifier::with_jwks_url(
            &Url::parse(ISSUER).unwrap(),
            "portal-api",
            Url::parse(&format!("{}/certs", realm.uri())).unwrap(),
        );
        let started = Instant::now();
        let refreshed = v.refresh().await;
        assert!(
            matches!(refreshed, Err(ApiError::Unavailable(_))),
            "{refreshed:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the request was held for {:?}",
            started.elapsed()
        );
    }

    /// AP-27: the issuer is stored without its trailing slash, and the comparison is exact
    /// afterwards. Both spellings of the realm URL build the same verifier, and a token whose
    /// `iss` is the other spelling — or a realm one path segment away — is refused.
    #[tokio::test]
    async fn the_issuer_is_stored_without_its_slash_and_matched_exactly_afterwards() {
        let (signer, set) = keypair("k1");
        for spelling in [ISSUER, &format!("{ISSUER}/")] {
            let v = BearerVerifier::with_jwks_url(
                &Url::parse(spelling).unwrap(),
                "portal-api",
                Url::parse("http://127.0.0.1:9/certs").unwrap(),
            );
            v.install(&set);
            assert_eq!(v.issuer, ISSUER, "{spelling}");
            assert!(
                v.verify(&sign(&signer, "k1", &claims(now() + 60)))
                    .await
                    .is_ok(),
                "{spelling} refused the realm's own token"
            );
            for other in [
                format!("{ISSUER}/"),
                format!("{ISSUER}/sub"),
                ISSUER.replace("/dev", "/prod"),
                ISSUER.replace("https", "http"),
                ISSUER.to_uppercase(),
            ] {
                let mut foreign = claims(now() + 60);
                foreign["iss"] = json!(other);
                assert!(
                    matches!(
                        v.verify(&sign(&signer, "k1", &foreign)).await,
                        Err(ApiError::Unauthorized)
                    ),
                    "{spelling} accepted a token issued by {other}"
                );
            }
        }
    }

    /// AP-28, PF-46: the verifier is bound to one audience. A token minted for another client of
    /// the same realm is refused, and only the caller that asks for another audience by name —
    /// the internal listener — gets one.
    #[tokio::test]
    async fn the_audience_is_the_one_the_verifier_was_built_with() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        v.install(&set);

        for foreign in ["portal-internal", "broker", "portal-api ", "Portal-Api", ""] {
            let mut other = claims(now() + 60);
            other["aud"] = json!(foreign);
            assert!(
                matches!(
                    v.verify(&sign(&signer, "k1", &other)).await,
                    Err(ApiError::Unauthorized)
                ),
                "a token for {foreign:?} was taken as one for portal-api"
            );
        }

        // The same verifier, asked for the internal audience by name, takes that one and still
        // refuses the Portal's own.
        let mut internal = claims(now() + 60);
        internal["aud"] = json!("portal-internal");
        let token = sign(&signer, "k1", &internal);
        assert!(v
            .verify_for_audience(&token, "portal-internal")
            .await
            .is_ok());
        assert!(matches!(
            v.verify_for_audience(&sign(&signer, "k1", &claims(now() + 60)), "portal-internal")
                .await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// AP-27: a verifier starts with no keys, and two verifiers share none. A token signed by a
    /// key one of them installed is nothing to the other.
    #[tokio::test]
    async fn a_verifier_starts_empty_and_shares_its_keys_with_no_other() {
        let (signer, set) = keypair("k1");
        let first = verifier();
        let second = verifier();
        assert!(first.key("k1").is_none(), "a fresh verifier had a key");
        assert_eq!(first.install(&set), 1);
        assert!(second.key("k1").is_none(), "the cache is shared");
        assert!(matches!(
            second
                .verify(&sign(&signer, "k1", &claims(now() + 60)))
                .await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// AP-27: the JWKS URL is taken as it is given, host and all — the realm's address inside the
    /// cluster is not the issuer the tokens name, and `new` is the caller that derives one from
    /// the other. Building a verifier is configuration and never panics, whatever the URL is.
    #[test]
    fn the_jwks_url_is_taken_as_given_and_building_one_never_panics() {
        for jwks in [
            "http://keycloak.identity.svc.cluster.local:8080/realms/dev/protocol/openid-connect/certs",
            "https://idm.example.test/realms/dev/protocol/openid-connect/certs",
            "http://127.0.0.1:9/certs",
            "https://idm.example.test/certs?x=1#f",
        ] {
            let url = Url::parse(jwks).unwrap();
            let v = BearerVerifier::with_jwks_url(
                &Url::parse(ISSUER).unwrap(),
                "portal-api",
                url.clone(),
            );
            assert_eq!(v.jwks_url, url);
            assert_eq!(v.issuer, ISSUER);
            assert_eq!(v.audience, "portal-api");
        }
    }

    // ---------------------------------------------------------------------------------------
    // T-2090 `install`: which keys of a realm's key set become verification keys.
    // ---------------------------------------------------------------------------------------

    /// A fresh RSA key pair: the signing key and its public half as a JWK, the way Keycloak lists
    /// the `edge` client's key (ADR-N-019).
    fn rsa_keypair(kid: &str) -> (EncodingKey, serde_json::Value) {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;
        use rsa::pkcs1::EncodeRsaPrivateKey as _;
        use rsa::traits::PublicKeyParts as _;

        let key = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).expect("an rsa key");
        let jwk = json!({
            "kty": "RSA",
            "kid": kid,
            "alg": "RS256",
            "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
            "e": URL_SAFE_NO_PAD.encode(key.e().to_bytes_be()),
        });
        (
            EncodingKey::from_rsa_der(key.to_pkcs1_der().expect("der").as_bytes()),
            jwk,
        )
    }

    fn set_of(jwks: Vec<serde_json::Value>) -> JwkSet {
        serde_json::from_value(json!({ "keys": jwks })).expect("a key set")
    }

    fn jwk_of(set: &JwkSet) -> serde_json::Value {
        serde_json::to_value(&set.keys[0]).expect("a jwk")
    }

    /// AP-27: a rotated realm replaces the keys it publishes. `install` replaces the cache and
    /// never merges, so a key the realm withdrew stops verifying the moment the new set arrives.
    #[tokio::test]
    async fn install_replaces_the_cache_so_a_withdrawn_key_stops_verifying() {
        let v = verifier();
        let (old_signer, old_set) = keypair("k-old");
        let (_new_signer, new_set) = keypair("k-new");
        assert_eq!(v.install(&old_set), 1);
        let old_token = sign(&old_signer, "k-old", &claims(now() + 60));
        assert!(v.verify(&old_token).await.is_ok());

        assert_eq!(v.install(&new_set), 1);
        assert!(v.key("k-old").is_none(), "the withdrawn key was kept");
        assert!(matches!(
            v.verify(&old_token).await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// AP-27: a realm that publishes an empty set has no usable key, and the Portal then verifies
    /// nothing rather than keeping yesterday's. It fails closed.
    #[tokio::test]
    async fn an_empty_or_unusable_key_set_empties_the_cache() {
        let v = verifier();
        let (signer, set) = keypair("k1");
        assert_eq!(v.install(&set), 1);

        assert_eq!(v.install(&set_of(vec![])), 0);
        assert!(v.key("k1").is_none());
        assert!(matches!(
            v.verify(&sign(&signer, "k1", &claims(now() + 60))).await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// AP-27: a key with no `kid` cannot be looked up by a token's header, so it is dropped
    /// rather than installed under an empty name that a header could claim.
    #[test]
    fn a_key_without_a_kid_is_dropped() {
        let v = verifier();
        let (_signer, set) = keypair("k1");
        let mut jwk = jwk_of(&set);
        jwk.as_object_mut().expect("an object").remove("kid");
        assert_eq!(v.install(&set_of(vec![jwk])), 0);
        assert!(v.key("").is_none());
    }

    /// AP-27: two keys under one `kid` collapse to one entry — the last one the realm listed —
    /// and the count says how many keys are cached, not how many were offered.
    #[tokio::test]
    async fn two_keys_under_one_kid_collapse_and_the_count_is_what_is_cached() {
        let v = verifier();
        let (first_signer, first) = keypair("k1");
        let (second_signer, second) = keypair("k1");
        assert_eq!(v.install(&set_of(vec![jwk_of(&first), jwk_of(&second)])), 1);

        let by_second = v
            .verify(&sign(&second_signer, "k1", &claims(now() + 60)))
            .await;
        let by_first = v
            .verify(&sign(&first_signer, "k1", &claims(now() + 60)))
            .await;
        assert!(by_second.is_ok(), "the last key listed is the cached one");
        assert!(matches!(by_first, Err(ApiError::Unauthorized)));
    }

    /// TR-03187 AR-11, ADR-N-019: the realm signs ES256 and the `edge` client RS256. Both install
    /// from one set, each verifies its own family, and neither verifies the other's.
    #[tokio::test]
    async fn an_es256_and_an_rs256_key_install_together_and_neither_verifies_the_others_family() {
        let v = verifier();
        let (ec_signer, ec_set) = keypair("realm-es256");
        let (rsa_signer, rsa_jwk) = rsa_keypair("edge-rs256");
        assert_eq!(v.install(&set_of(vec![jwk_of(&ec_set), rsa_jwk])), 2);

        assert!(v
            .verify(&sign(&ec_signer, "realm-es256", &claims(now() + 60)))
            .await
            .is_ok());

        let mut rs_header = Header::new(Algorithm::RS256);
        rs_header.kid = Some("edge-rs256".into());
        let rs_token = encode(&rs_header, &claims(now() + 60), &rsa_signer).unwrap();
        assert!(v.verify(&rs_token).await.is_ok());

        // The RS256 header naming the EC key, and the ES256 header naming the RSA key: the key
        // family is checked, so a header cannot borrow the other key.
        let mut crossed = Header::new(Algorithm::RS256);
        crossed.kid = Some("realm-es256".into());
        let crossed = encode(&crossed, &claims(now() + 60), &rsa_signer).unwrap();
        assert!(matches!(
            v.verify(&crossed).await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            v.verify(&sign(&ec_signer, "edge-rs256", &claims(now() + 60)))
                .await,
            Err(ApiError::Unauthorized)
        ));
    }

    /// AP-27: a signature algorithm outside ES256 and RS256 is not installed, whichever family it
    /// belongs to — a longer curve or a longer RSA hash is still a key the Portal never agreed to
    /// verify.
    #[test]
    fn a_key_whose_alg_is_neither_es256_nor_rs256_is_dropped() {
        let v = verifier();
        let (_signer, set) = keypair("k1");
        for alg in [
            "ES384", "ES512", "RS384", "RS512", "PS256", "HS256", "EdDSA",
        ] {
            let mut jwk = jwk_of(&set);
            jwk["alg"] = json!(alg);
            jwk["kid"] = json!("k-other");
            let Ok(other) = serde_json::from_value::<JwkSet>(json!({ "keys": [jwk] })) else {
                continue; // the library does not know the name at all, which is refusal enough
            };
            assert_eq!(v.install(&other), 0, "{alg} was installed");
        }
    }

    /// AP-27: Keycloak lists a key without `alg` in some realm configurations; the key type then
    /// decides, and an EC or RSA key installs while anything else does not.
    #[test]
    fn a_key_without_an_alg_installs_by_its_key_type() {
        let v = verifier();
        let (_signer, set) = keypair("k1");
        let mut jwk = jwk_of(&set);
        jwk.as_object_mut().expect("an object").remove("alg");
        assert_eq!(v.install(&set_of(vec![jwk])), 1, "an EC key with no alg");

        let (_rsa_signer, mut rsa_jwk) = rsa_keypair("k-rsa");
        rsa_jwk.as_object_mut().expect("an object").remove("alg");
        assert_eq!(
            v.install(&set_of(vec![rsa_jwk])),
            1,
            "an RSA key with no alg"
        );
    }

    /// AP-27: a key of the right algorithm whose material does not parse is dropped, and the keys
    /// beside it in the same set still install — one broken entry does not take the realm down.
    #[test]
    fn a_key_that_does_not_parse_is_dropped_and_the_rest_of_the_set_installs() {
        let v = verifier();
        let (_signer, set) = keypair("k-good");
        let mut broken = jwk_of(&set);
        broken["kid"] = json!("k-broken");
        broken["x"] = json!("!!!not base64!!!");
        assert_eq!(v.install(&set_of(vec![broken, jwk_of(&set)])), 1);
        assert!(v.key("k-good").is_some());
        assert!(v.key("k-broken").is_none());
    }

    /// AP-27: a realm may list many keys after a rotation; all of them install and the count is
    /// the number cached.
    #[test]
    fn a_long_key_set_installs_every_usable_key() {
        let v = verifier();
        let jwks: Vec<serde_json::Value> = (0..32)
            .map(|i| {
                let (_signer, set) = keypair(&format!("k{i}"));
                jwk_of(&set)
            })
            .collect();
        assert_eq!(v.install(&set_of(jwks)), 32);
        assert!(v.key("k0").is_some() && v.key("k31").is_some());
        assert!(v.key("k32").is_none());
    }

    /// AP-27: installing the same set twice leaves the same cache — a refresh that repeats does
    /// not double anything or drop a key that is still published.
    #[test]
    fn installing_the_same_set_twice_leaves_the_same_cache() {
        let v = verifier();
        let (_signer, set) = keypair("k1");
        assert_eq!(v.install(&set), 1);
        assert_eq!(v.install(&set), 1);
        assert!(v.key("k1").is_some());
    }

    #[test]
    fn jwks_url_is_the_realm_certs_endpoint() {
        let v = BearerVerifier::new(
            &Url::parse("https://idm.example.test/realms/dev/").unwrap(),
            "portal-api",
        );
        assert_eq!(
            v.jwks_url.as_str(),
            "https://idm.example.test/realms/dev/protocol/openid-connect/certs"
        );
        assert_eq!(v.issuer, ISSUER);
    }
}
