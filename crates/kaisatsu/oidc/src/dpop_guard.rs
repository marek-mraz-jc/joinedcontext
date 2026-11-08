#![forbid(unsafe_code)]
//! Shared resource-server DPoP and Bearer access-token verifier for UserInfo, Admin Compat,
//! Native Admin, OData, MCP, and SCIM (RFC 9449 §4.3, §7.1, §9, K6).

use crate::OidcState;
use crate::dpop::{
    issue_dpop_nonce, realm_requires_dpop_nonce, verify_dpop_nonce, verify_dpop_proof,
};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use jc_router::Mount;
use kaisatsu_core::{KaisatsuError, Result, RevokedTokenRecord};
use kaisatsu_crypto::sha256_hex;
use serde_json::Value;

/// Builds the external HTU (`scheme://host/prefix/path`) for the current request behind `mount`.
#[must_use]
pub fn external_htu(mount: &Mount, path: &str) -> String {
    format!(
        "{}://{}{}{}",
        mount.scheme, mount.host, mount.path_prefix, path
    )
}

/// Extracts `(is_dpop_scheme, raw_token)` from the `Authorization` header.
pub fn parse_authz_header(headers: &HeaderMap) -> Result<(bool, &str)> {
    let authz = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| KaisatsuError::Unauthorized("missing authorization header".into()))?;
    if let Some(rest) = authz.strip_prefix("DPoP ") {
        let tok = rest.trim();
        if tok.is_empty() {
            return Err(KaisatsuError::Unauthorized("empty DPoP token".into()));
        }
        Ok((true, tok))
    } else if let Some(rest) = authz.strip_prefix("Bearer ") {
        let tok = rest.trim();
        if tok.is_empty() {
            return Err(KaisatsuError::Unauthorized("empty Bearer token".into()));
        }
        Ok((false, tok))
    } else {
        Err(KaisatsuError::Unauthorized(
            "unsupported authorization scheme".into(),
        ))
    }
}

/// Enforces DPoP token binding (`cnf.jkt`), proof verification (`htm`, `htu`, `ath`, `jkt`),
/// single-use `jti` replay protection, and optional resource-server DPoP nonce (RFC 9449 §7.1 & §9).
#[allow(clippy::too_many_arguments)]
pub async fn verify_resource_dpop(
    state: &OidcState,
    realm_id: &str,
    claims: &Value,
    headers: &HeaderMap,
    method: &str,
    htu: &str,
    raw_token: &str,
    dpop_scheme: bool,
) -> Result<Option<String>> {
    let Some(bound_jkt) = claims.pointer("/cnf/jkt").and_then(Value::as_str) else {
        if dpop_scheme {
            return Err(KaisatsuError::Unauthorized(
                "the DPoP scheme needs a DPoP-bound token".into(),
            ));
        }
        return Ok(None);
    };
    if !dpop_scheme {
        return Err(KaisatsuError::Unauthorized(
            "a DPoP-bound token needs the DPoP scheme".into(),
        ));
    }
    let dpop_values: Vec<_> = headers.get_all("dpop").iter().collect();
    if dpop_values.len() != 1 {
        return Err(KaisatsuError::Unauthorized(
            "exactly one DPoP proof header is required".into(),
        ));
    }
    let proof_hdr = dpop_values[0]
        .to_str()
        .map_err(|_| KaisatsuError::Unauthorized("invalid DPoP header".into()))?;
    let proof = verify_dpop_proof(proof_hdr.trim(), method, htu, Some(raw_token), None)
        .map_err(|_| KaisatsuError::Unauthorized("invalid DPoP proof".into()))?;
    if proof.jkt != bound_jkt {
        return Err(KaisatsuError::Unauthorized(
            "DPoP proof key does not match the token".into(),
        ));
    }

    let now = Utc::now().timestamp();
    let fresh_nonce = issue_dpop_nonce(&state.kek, realm_id, "rs", now);
    if let Some(realm_cfg) = state.store.get_realm(realm_id).await?
        && realm_requires_dpop_nonce(&realm_cfg)
    {
        let valid_nonce = proof
            .nonce
            .as_deref()
            .is_some_and(|n| verify_dpop_nonce(&state.kek, &realm_cfg.id, "rs", n, now));
        if !valid_nonce {
            return Err(KaisatsuError::Unauthorized(format!(
                "use_dpop_nonce:{fresh_nonce}"
            )));
        }
    }

    if proof.jti.is_empty() || proof.jti.len() > 512 {
        return Err(KaisatsuError::Unauthorized("invalid DPoP jti".into()));
    }
    state
        .store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex(&format!("dpop:{}", proof.jti)),
            realm_id: realm_id.to_owned(),
            expires_at: Utc::now() + Duration::minutes(10),
        })
        .await
        .map_err(|_| KaisatsuError::Unauthorized("replayed DPoP proof".into()))?;

    Ok(Some(fresh_nonce))
}

/// Converts a resource DPoP `use_dpop_nonce:{nonce}` error into an RFC 9449 §9 `401 Unauthorized`
/// response with `WWW-Authenticate: DPoP error="use_dpop_nonce"` and `DPoP-Nonce`.
pub fn into_dpop_aware_response(err: KaisatsuError) -> Response {
    if let KaisatsuError::Unauthorized(ref msg) = err
        && let Some(nonce) = msg.strip_prefix("use_dpop_nonce:")
    {
        return (
            StatusCode::UNAUTHORIZED,
            [
                (
                    header::WWW_AUTHENTICATE,
                    r#"DPoP error="use_dpop_nonce", error_description="Resource server requires nonce in DPoP proof", algs="ES256 RS256 EdDSA""#.to_owned(),
                ),
                (header::HeaderName::from_static("dpop-nonce"), nonce.to_owned()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            r#"{"error":"use_dpop_nonce"}"#,
        )
            .into_response();
    }
    err.into_response()
}
