#![forbid(unsafe_code)]
//! Upstream OIDC identity provider broker and first-broker-login account linking (W7, W11).

use crate::guard::OutboundGuard;
use kaisatsu_core::{
    IdentityProviderRepresentation, KaisatsuError, Result, Store, UserRepresentation,
};
use kaisatsu_crypto::{sha256_base64url, verify_jwt_with_jwk};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamBrokerRedirect {
    pub authorization_url: String,
    pub state: String,
    pub pkce_verifier: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamIdTokenClaims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
}

/// User attribute holding the upstream `sub` an account is linked to for one IdP alias.
#[must_use]
pub fn idp_link_attribute(alias: &str) -> String {
    format!("kaisatsu.idp.{alias}.sub")
}

pub struct UpstreamOidcBroker {
    guard: OutboundGuard,
}

impl UpstreamOidcBroker {
    #[must_use]
    pub fn new(guard: OutboundGuard) -> Self {
        Self { guard }
    }

    /// Builds the authorization redirect URL with PKCE S256 for an upstream OIDC IdentityProvider (W11).
    pub fn start_login(
        &self,
        idp: &IdentityProviderRepresentation,
        callback_url: &str,
        state: &str,
        nonce: &str,
        pkce_verifier: &str,
    ) -> Result<UpstreamBrokerRedirect> {
        let auth_url = idp
            .config
            .get("authorizationUrl")
            .ok_or_else(|| KaisatsuError::BadRequest("IdP missing authorizationUrl".into()))?;
        let client_id = idp
            .config
            .get("clientId")
            .ok_or_else(|| KaisatsuError::BadRequest("IdP missing clientId".into()))?;
        let challenge = sha256_base64url(pkce_verifier);
        let mut parsed = url::Url::parse(auth_url)
            .map_err(|_| KaisatsuError::BadRequest("invalid IdP authorizationUrl".into()))?;
        parsed
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", callback_url)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid profile email")
            .append_pair("state", state)
            .append_pair("nonce", nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(UpstreamBrokerRedirect {
            authorization_url: parsed.to_string(),
            state: state.to_owned(),
            pkce_verifier: pkce_verifier.to_owned(),
            nonce: nonce.to_owned(),
        })
    }

    /// Verifies the upstream ID token against its JWKS (issuer, audience, and the `nonce` this
    /// login sent) and executes W7's first broker login: creates a local account by verified
    /// e-mail only when `idp.trust_email` is true (GHSA-c7xw-p58w-h6fj protection). An account
    /// this IdP created earlier is recognized by its link attribute; an existing local account
    /// with the same e-mail is never linked silently (Keycloak asks the person to confirm and
    /// re-authenticate first), it answers `Conflict`.
    pub async fn complete_first_broker_login(
        &self,
        store: &Arc<dyn Store>,
        realm_id: &str,
        idp: &IdentityProviderRepresentation,
        id_token: &str,
        expected_nonce: &str,
    ) -> Result<UserRepresentation> {
        let jwks_url = idp
            .config
            .get("jwksUrl")
            .ok_or_else(|| KaisatsuError::BadRequest("IdP missing jwksUrl".into()))?;
        let issuer = idp.config.get("issuer").map(String::as_str);
        let client_id = idp.config.get("clientId").map(String::as_str);

        let jwks: Value = self.guard.get_json(jwks_url, None, None).await?;
        let keys = jwks
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| KaisatsuError::Unauthorized("upstream JWKS has no keys".into()))?;

        let mut verified_claims: Option<UpstreamIdTokenClaims> = None;
        for jwk in keys {
            if let Ok((_, claims)) =
                verify_jwt_with_jwk::<UpstreamIdTokenClaims>(id_token, jwk, issuer, client_id)
            {
                verified_claims = Some(claims);
                break;
            }
        }
        let claims = verified_claims.ok_or_else(|| {
            KaisatsuError::Unauthorized("upstream ID token did not verify against JWKS".into())
        })?;
        if expected_nonce.is_empty() || claims.nonce.as_deref() != Some(expected_nonce) {
            return Err(KaisatsuError::Unauthorized(
                "upstream ID token nonce does not match this login".into(),
            ));
        }
        let link_key = idp_link_attribute(&idp.alias);

        // W7, research-keycloak §5.3: link or create by verified e-mail only when IdP is trusted
        if !idp.trust_email || claims.email_verified != Some(true) {
            return Err(KaisatsuError::Forbidden(
                "upstream identity provider email is not verified or not trusted for linking"
                    .into(),
            ));
        }
        let email = claims
            .email
            .as_deref()
            .filter(|e| !e.is_empty())
            .ok_or_else(|| KaisatsuError::Forbidden("upstream token carries no email".into()))?;

        if let Some(existing) = store
            .get_user_by_username_or_email(realm_id, None, email)
            .await?
        {
            let linked = existing
                .attributes
                .get(&link_key)
                .is_some_and(|v| v.iter().any(|s| s == &claims.sub));
            if linked && existing.enabled {
                return Ok(existing);
            }
            return Err(KaisatsuError::Conflict(
                "an account with this e-mail exists; it must confirm the link by signing in".into(),
            ));
        }

        let created = store
            .create_user(UserRepresentation {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm_id.to_owned(),
                org_id: idp.org_id.clone(),
                username: email.to_ascii_lowercase(),
                email: Some(email.to_ascii_lowercase()),
                first_name: claims.given_name,
                last_name: claims.family_name,
                enabled: true,
                email_verified: true,
                attributes: std::collections::HashMap::from([(link_key, vec![claims.sub.clone()])]),
                ..Default::default()
            })
            .await?;
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaisatsu_crypto::{Kek, KeyManager, sign_jwt};
    use kaisatsu_store_memory::MemoryStore;
    use serde_json::json;
    use std::collections::BTreeMap;

    const JWKS: &str = "https://login.upstream.example/keys";
    const ISS: &str = "https://login.upstream.example";

    fn idp(trust_email: bool) -> IdentityProviderRepresentation {
        IdentityProviderRepresentation {
            alias: "upstream".into(),
            trust_email,
            config: BTreeMap::from([
                (
                    "authorizationUrl".into(),
                    format!("{ISS}/authorize?tenant=t1"),
                ),
                ("clientId".into(), "kaisatsu-rp".into()),
                ("jwksUrl".into(), JWKS.into()),
                ("issuer".into(), ISS.into()),
            ]),
            ..Default::default()
        }
    }

    struct Upstream {
        key: kaisatsu_core::SigningKeyRecord,
        kek: Kek,
        broker: UpstreamOidcBroker,
    }

    fn upstream() -> Upstream {
        let kek = Kek::generate_for_tests();
        let key = KeyManager::generate_es256("upstream", &kek, 100, "ACTIVE").unwrap();
        let guard = OutboundGuard::default();
        let jwk: Value = serde_json::from_str(&key.public_jwk_json).unwrap();
        guard.insert_mock(JWKS, 200, json!({ "keys": [jwk] }).to_string());
        Upstream {
            key,
            kek,
            broker: UpstreamOidcBroker::new(guard),
        }
    }

    impl Upstream {
        fn id_token(&self, extra: Value) -> String {
            let now = chrono::Utc::now().timestamp();
            let mut c = json!({ "iss": ISS, "aud": "kaisatsu-rp", "sub": "up-1", "iat": now,
                "exp": now + 300, "nonce": "n-1", "email": "Ada@Hel.fi", "email_verified": true,
                "given_name": "Ada" });
            for (k, v) in extra.as_object().unwrap() {
                c[k] = v.clone();
            }
            sign_jwt(&self.key, &self.kek, Some("JWT"), &c).unwrap()
        }
    }

    fn store() -> Arc<dyn Store> {
        Arc::new(MemoryStore::new(Kek::generate_for_tests()))
    }

    #[test]
    fn start_login_sends_pkce_s256_state_and_nonce() {
        let b = UpstreamOidcBroker::new(OutboundGuard::default());
        let r = b
            .start_login(
                &idp(true),
                "https://idm.test/cb",
                "st-1",
                "n-1",
                "verifier-123",
            )
            .unwrap();
        let url = url::Url::parse(&r.authorization_url).unwrap();
        let q: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["tenant"], "t1");
        assert_eq!(q["client_id"], "kaisatsu-rp");
        assert_eq!(q["redirect_uri"], "https://idm.test/cb");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["state"], "st-1");
        assert_eq!(q["nonce"], "n-1");
        assert_eq!(q["code_challenge"], sha256_base64url("verifier-123"));
        assert_eq!(q["code_challenge_method"], "S256");
        let mut broken = idp(true);
        broken.config.remove("clientId");
        assert!(
            b.start_login(&broken, "https://idm.test/cb", "s", "n", "v")
                .is_err()
        );
        broken.config.insert("clientId".into(), "x".into());
        broken.config.remove("authorizationUrl");
        assert!(
            b.start_login(&broken, "https://idm.test/cb", "s", "n", "v")
                .is_err()
        );
        broken
            .config
            .insert("authorizationUrl".into(), "not a url".into());
        assert!(
            b.start_login(&broken, "https://idm.test/cb", "s", "n", "v")
                .is_err()
        );
    }

    #[tokio::test]
    async fn first_login_creates_a_linked_account_and_recognizes_it_again() {
        let up = upstream();
        let store = store();
        let token = up.id_token(json!({}));
        let user = up
            .broker
            .complete_first_broker_login(&store, "dev", &idp(true), &token, "n-1")
            .await
            .unwrap();
        assert_eq!(user.username, "ada@hel.fi");
        assert_eq!(user.first_name.as_deref(), Some("Ada"));
        assert_eq!(
            user.attributes[&idp_link_attribute("upstream")],
            vec!["up-1"]
        );
        let again = up
            .broker
            .complete_first_broker_login(&store, "dev", &idp(true), &up.id_token(json!({})), "n-1")
            .await
            .unwrap();
        assert_eq!(again.id, user.id);
    }

    #[tokio::test]
    async fn an_existing_local_account_is_never_taken_over_by_email() {
        let up = upstream();
        let store = store();
        store
            .create_user(UserRepresentation {
                id: "local-1".into(),
                realm_id: "dev".into(),
                username: "ada@hel.fi".into(),
                email: Some("ada@hel.fi".into()),
                enabled: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let e = up
            .broker
            .complete_first_broker_login(&store, "dev", &idp(true), &up.id_token(json!({})), "n-1")
            .await
            .unwrap_err();
        assert!(matches!(e, KaisatsuError::Conflict(_)), "{e:?}");
    }

    #[tokio::test]
    async fn unverified_untrusted_forged_or_replayed_tokens_are_refused() {
        let up = upstream();
        let other = upstream();
        let store = store();
        let cases = [
            (idp(false), up.id_token(json!({})), "n-1"),
            (
                idp(true),
                up.id_token(json!({ "email_verified": false })),
                "n-1",
            ),
            (idp(true), up.id_token(json!({ "email": "" })), "n-1"),
            (idp(true), up.id_token(json!({ "nonce": "n-other" })), "n-1"),
            (idp(true), up.id_token(json!({})), ""),
            (
                idp(true),
                up.id_token(json!({ "aud": "someone-else" })),
                "n-1",
            ),
            (
                idp(true),
                up.id_token(json!({ "iss": "https://evil.example" })),
                "n-1",
            ),
            (idp(true), other.id_token(json!({})), "n-1"),
        ];
        for (i, (p, token, nonce)) in cases.into_iter().enumerate() {
            assert!(
                up.broker
                    .complete_first_broker_login(&store, "dev", &p, &token, nonce)
                    .await
                    .is_err(),
                "case {i} was accepted"
            );
        }
        assert!(
            store
                .get_user_by_username_or_email("dev", None, "ada@hel.fi")
                .await
                .unwrap()
                .is_none()
        );
        // A JWKS endpoint that fails or carries no keys.
        let failing = OutboundGuard::default();
        failing.insert_mock(JWKS, 500, "{}");
        let b = UpstreamOidcBroker::new(failing);
        assert!(
            b.complete_first_broker_login(
                &store,
                "dev",
                &idp(true),
                &up.id_token(json!({})),
                "n-1"
            )
            .await
            .is_err()
        );
        let empty = OutboundGuard::default();
        empty.insert_mock(JWKS, 200, "{}");
        let b = UpstreamOidcBroker::new(empty);
        assert!(
            b.complete_first_broker_login(
                &store,
                "dev",
                &idp(true),
                &up.id_token(json!({})),
                "n-1"
            )
            .await
            .is_err()
        );
        let mut no_jwks = idp(true);
        no_jwks.config.remove("jwksUrl");
        assert!(
            up.broker
                .complete_first_broker_login(
                    &store,
                    "dev",
                    &no_jwks,
                    &up.id_token(json!({})),
                    "n-1"
                )
                .await
                .is_err()
        );
    }
}
