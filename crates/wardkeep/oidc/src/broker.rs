#![forbid(unsafe_code)]
//! Upstream OIDC identity provider broker and first-broker-login account linking (W7, W11).

use crate::guard::OutboundGuard;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use wardkeep_core::{
    IdentityProviderRepresentation, Result, Store, UserRepresentation, WardkeepError,
};
use wardkeep_crypto::{sha256_base64url, verify_jwt_with_jwk};

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
            .ok_or_else(|| WardkeepError::BadRequest("IdP missing authorizationUrl".into()))?;
        let client_id = idp
            .config
            .get("clientId")
            .ok_or_else(|| WardkeepError::BadRequest("IdP missing clientId".into()))?;
        let challenge = sha256_base64url(pkce_verifier);
        let mut parsed = url::Url::parse(auth_url)
            .map_err(|_| WardkeepError::BadRequest("invalid IdP authorizationUrl".into()))?;
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

    /// Verifies the upstream ID token against its JWKS and executes W7's first broker login
    /// (links or creates a local account by verified e-mail only when `idp.trust_email` is true,
    /// GHSA-c7xw-p58w-h6fj protection).
    pub async fn complete_first_broker_login(
        &self,
        store: &Arc<dyn Store>,
        realm_id: &str,
        idp: &IdentityProviderRepresentation,
        id_token: &str,
    ) -> Result<UserRepresentation> {
        let jwks_url = idp
            .config
            .get("jwksUrl")
            .ok_or_else(|| WardkeepError::BadRequest("IdP missing jwksUrl".into()))?;
        let issuer = idp.config.get("issuer").map(String::as_str);
        let client_id = idp.config.get("clientId").map(String::as_str);

        let jwks: Value = self.guard.get_json(jwks_url, None, None).await?;
        let keys = jwks
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| WardkeepError::Unauthorized("upstream JWKS has no keys".into()))?;

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
            WardkeepError::Unauthorized("upstream ID token did not verify against JWKS".into())
        })?;

        // W7, research-keycloak §5.3: link or create by verified e-mail only when IdP is trusted
        if !idp.trust_email || claims.email_verified != Some(true) {
            return Err(WardkeepError::Forbidden(
                "upstream identity provider email is not verified or not trusted for linking".into(),
            ));
        }
        let email = claims
            .email
            .as_deref()
            .filter(|e| !e.is_empty())
            .ok_or_else(|| WardkeepError::Forbidden("upstream token carries no email".into()))?;

        if let Some(existing) = store
            .get_user_by_username_or_email(realm_id, None, email)
            .await?
        {
            return Ok(existing);
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
                ..Default::default()
            })
            .await?;
        Ok(created)
    }
}
