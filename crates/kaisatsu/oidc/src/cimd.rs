#![forbid(unsafe_code)]
//! Client ID Metadata Document (CIMD, W1):
//! - Client ID is an `https://` URL on one of the realm's `cimd_allowed_client_domains`
//!   (`claude.ai`, `vscode.dev`).
//! - Fetched through [`crate::guard::OutboundGuard`].
//! - Refused on any field outside RFC 7591 + the CIMD draft (`#[serde(deny_unknown_fields)]`).
//! - Redirect URIs must be exact (no `*`) and on `cimd_allowed_redirect_domains`
//!   (`claude.ai`, `vscode.dev`, `code.visualstudio.com`, `localhost`, `127.0.0.1`).
//! - Resulting client is public, `full_scope_allowed: false`, and gets only `default_optional_client_scopes`
//!   (`connector`, `offline_access`) plus `cimd_optional_client_scopes` (`mcp:portal`, `offline_access`).

use crate::guard::OutboundGuard;
use kaisatsu_core::{ClientRepresentation, KaisatsuError, RealmConfig, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::Url;

/// RFC 7591 + draft-ietf-oauth-client-id-metadata-document fields only (`deny_unknown_fields`, W1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CimdMetadata {
    pub client_id: String,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub client_uri: Option<String>,
    #[serde(default)]
    pub logo_uri: Option<String>,
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub response_types: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub contacts: Vec<String>,
    #[serde(default)]
    pub tos_uri: Option<String>,
    #[serde(default)]
    pub policy_uri: Option<String>,
    #[serde(default)]
    pub jwks_uri: Option<String>,
    #[serde(default)]
    pub jwks: Option<serde_json::Value>,
    #[serde(default)]
    pub software_id: Option<String>,
    #[serde(default)]
    pub software_version: Option<String>,
    #[serde(default)]
    pub application_type: Option<String>,
}

/// Validates `doc` against `expected_client_id` and `realm`'s CIMD policy, returning a
/// [`ClientRepresentation`] with `full_scope_allowed = false` and `consent_required = true` (W1).
pub fn validate_and_build_cimd_client(
    realm: &RealmConfig,
    expected_client_id: &str,
    doc: CimdMetadata,
) -> Result<ClientRepresentation> {
    if doc.client_id != expected_client_id {
        return Err(KaisatsuError::invalid_client(
            "CIMD client_id does not match fetched URL",
        ));
    }
    let client_url = Url::parse(expected_client_id)
        .map_err(|_| KaisatsuError::invalid_client("CIMD client_id is not a valid URL"))?;
    if client_url.scheme() != "https" {
        return Err(KaisatsuError::invalid_client(
            "CIMD client_id must use https",
        ));
    }
    let client_host = client_url
        .host_str()
        .ok_or_else(|| KaisatsuError::invalid_client("CIMD client_id has no host"))?;
    if !realm
        .cimd_allowed_client_domains
        .iter()
        .any(|d| d.eq_ignore_ascii_case(client_host))
    {
        return Err(KaisatsuError::invalid_client(format!(
            "CIMD client host '{client_host}' is not permitted by realm policy"
        )));
    }

    if doc.redirect_uris.is_empty() {
        return Err(KaisatsuError::invalid_client(
            "CIMD metadata has no redirect_uris",
        ));
    }
    for uri in &doc.redirect_uris {
        if uri.contains('*') {
            return Err(KaisatsuError::invalid_client(
                "CIMD redirect_uri may not contain wildcards",
            ));
        }
        let parsed = Url::parse(uri)
            .map_err(|_| KaisatsuError::invalid_client("invalid redirect_uri in CIMD metadata"))?;
        let r_host = parsed
            .host_str()
            .ok_or_else(|| KaisatsuError::invalid_client("redirect_uri has no host"))?;
        let is_loopback = matches!(r_host, "localhost" | "127.0.0.1");
        if parsed.scheme() != "https" && !(is_loopback && parsed.scheme() == "http") {
            return Err(KaisatsuError::invalid_client(
                "CIMD redirect_uri must be https or loopback http",
            ));
        }
        if !realm
            .cimd_allowed_redirect_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(r_host))
        {
            return Err(KaisatsuError::invalid_client(format!(
                "CIMD redirect_uri host '{r_host}' is not permitted"
            )));
        }
    }

    if let Some(logo) = &doc.logo_uri {
        let parsed = Url::parse(logo)
            .map_err(|_| KaisatsuError::invalid_client("invalid logo_uri in CIMD metadata"))?;
        let l_host = parsed.host_str().unwrap_or_default();
        if parsed.scheme() != "https"
            || !realm
                .cimd_allowed_redirect_domains
                .iter()
                .any(|d| d.eq_ignore_ascii_case(l_host))
        {
            return Err(KaisatsuError::invalid_client(
                "CIMD logo_uri host is not permitted",
            ));
        }
    }

    let mut attributes = BTreeMap::new();
    attributes.insert("pkce.code.challenge.method".into(), "S256".into());

    Ok(ClientRepresentation {
        id: uuid::Uuid::now_v7().to_string(),
        realm_id: realm.id.clone(),
        org_id: None,
        client_id: doc.client_id,
        name: doc.client_name,
        description: Some("CIMD client".into()),
        enabled: true,
        protocol: "openid-connect".into(),
        public_client: true,
        bearer_only: false,
        consent_required: true,
        standard_flow_enabled: true,
        implicit_flow_enabled: false,
        direct_access_grants_enabled: false,
        service_accounts_enabled: false,
        authorization_services_enabled: false,
        frontchannel_logout: false,
        // W1, KNOWLEDGE T-5954: CIMD clients get fullScopeAllowed: false
        full_scope_allowed: false,
        client_authenticator_type: "client-secret".into(),
        root_url: None,
        base_url: doc.client_uri,
        redirect_uris: doc.redirect_uris,
        web_origins: Vec::new(),
        default_client_scopes: vec![
            "basic".into(),
            "email".into(),
            "profile".into(),
            "acr".into(),
        ],
        optional_client_scopes: cimd_optional_scopes(realm),
        attributes,
        protocol_mappers: Vec::new(),
        secret: None,
        managed_by: Some("cimd".into()),
        is_cimd: true,
    })
}

/// The realm's default optional scopes, then the CIMD-only ones, each once. A stored CIMD client
/// takes them from the realm at every resolve, so a change to either list reaches it at once.
pub fn cimd_optional_scopes(realm: &RealmConfig) -> Vec<String> {
    let mut scopes = realm.default_optional_client_scopes.clone();
    for s in &realm.cimd_optional_client_scopes {
        if !scopes.contains(s) {
            scopes.push(s.clone());
        }
    }
    scopes
}

/// Fetches and validates a CIMD document through `guard`.
pub async fn fetch_cimd_client(
    guard: &OutboundGuard,
    realm: &RealmConfig,
    client_id_url: &str,
) -> Result<ClientRepresentation> {
    let doc: CimdMetadata = guard
        .get_json(
            client_id_url,
            Some(&realm.cimd_allowed_client_domains),
            None,
        )
        .await
        .map_err(|_| KaisatsuError::invalid_client("Metadata fetch failed"))?;
    validate_and_build_cimd_client(realm, client_id_url, doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cimd_accepts_claude_and_refuses_unknown_fields_or_untrusted_hosts() {
        let realm = RealmConfig::default();
        let raw = serde_json::json!({
            "client_id": "https://claude.ai/oauth/mcp-oauth-client-metadata",
            "client_name": "Claude",
            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
            "grant_types": ["authorization_code", "refresh_token", "urn:ietf:params:oauth:grant-type:jwt-bearer"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        });
        let doc: CimdMetadata = serde_json::from_value(raw).expect("parse cimd");
        let client = validate_and_build_cimd_client(
            &realm,
            "https://claude.ai/oauth/mcp-oauth-client-metadata",
            doc,
        )
        .expect("valid cimd");
        assert!(client.public_client);
        assert!(!client.full_scope_allowed);
        assert_eq!(
            client.optional_client_scopes,
            vec!["connector", "offline_access", "mcp:portal"]
        );

        // A client_id host off `cimd_allowed_client_domains` is refused
        let off_list = "https://evil.example/oauth/mcp-oauth-client-metadata";
        let doc: CimdMetadata = serde_json::from_value(serde_json::json!({
            "client_id": off_list,
            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]
        }))
        .expect("parse cimd");
        let err = validate_and_build_cimd_client(&realm, off_list, doc).expect_err("off list");
        assert!(err.to_string().contains("not permitted"), "{err}");

        // Unknown field outside RFC 7591 + CIMD draft is refused
        let unknown_field = serde_json::json!({
            "client_id": "https://claude.ai/oauth/mcp-oauth-client-metadata",
            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
            "evil_extension": true
        });
        assert!(serde_json::from_value::<CimdMetadata>(unknown_field).is_err());
    }
}
