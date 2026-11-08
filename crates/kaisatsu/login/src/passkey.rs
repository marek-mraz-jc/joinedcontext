#![forbid(unsafe_code)]
//! WebAuthn passkey registration and authentication ceremony helpers for `kaisatsu-login`
//! and `kaisatsu-admin` (W3C WebAuthn Level 3 §7.1 & §7.2, W7, K11).

use chrono::Utc;
use jc_router::Mount;
use kaisatsu_core::{
    KaisatsuError, RealmConfig, Result, Store, UserRepresentation, WebAuthnCredentialRecord,
};
use kaisatsu_crypto::{
    AuthenticationClientResponse, COSE_ALG_EDDSA, COSE_ALG_ES256, COSE_ALG_RS256, CosePublicKey,
    RegistrationClientResponse, random_handle_256, verify_authentication_response,
    verify_registration_response,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// Returns the effective WebAuthn Relying Party ID for `realm` and `mount`.
#[must_use]
pub fn effective_rp_id(realm: &RealmConfig, mount: &Mount, passwordless: bool) -> String {
    let configured = if passwordless {
        realm.web_authn_policy_passwordless_rp_id.trim()
    } else {
        realm.web_authn_policy_rp_id.trim()
    };
    if !configured.is_empty() {
        return configured.to_ascii_lowercase();
    }
    mount
        .host
        .split_once(':')
        .map_or(mount.host.as_str(), |(h, _)| h)
        .to_ascii_lowercase()
}

/// Returns the allowed WebAuthn origins for `realm` and `mount`.
#[must_use]
pub fn allowed_origins_for_realm(realm: &RealmConfig, mount: &Mount) -> Vec<String> {
    let mut origins = vec![format!("{}://{}", mount.scheme, mount.host)];
    let rp_id = effective_rp_id(realm, mount, true);
    let https_rp = format!("https://{rp_id}");
    if !origins.contains(&https_rp) {
        origins.push(https_rp);
    }
    if let Some(extra) = realm.attributes.get("webauthn.allowed.origins") {
        for o in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if !origins.iter().any(|existing| existing == o) {
                origins.push(o.to_owned());
            }
        }
    }
    origins
}

/// Ensures `user` has a stable opaque `webauthn.user.handle` attribute (never the raw user ID, K11).
pub async fn ensure_user_handle(
    store: &Arc<dyn Store>,
    realm_id: &str,
    user: &mut UserRepresentation,
) -> Result<String> {
    if let Some(vals) = user.attributes.get("webauthn.user.handle")
        && let Some(first) = vals.first().filter(|s| !s.is_empty())
    {
        return Ok(first.clone());
    }
    let handle = random_handle_256();
    user.attributes
        .insert("webauthn.user.handle".into(), vec![handle.clone()]);
    let _ = store.update_user(user.clone()).await?;
    let _ = realm_id;
    Ok(handle)
}

/// Builds `PublicKeyCredentialCreationOptions` JSON for a registration ceremony (W3C WebAuthn L3 §7.1).
pub async fn build_creation_options(
    store: &Arc<dyn Store>,
    mount: &Mount,
    realm: &RealmConfig,
    user: &mut UserRepresentation,
    challenge_b64: &str,
) -> Result<Value> {
    let user_handle = ensure_user_handle(store, &realm.id, user).await?;
    let rp_id = effective_rp_id(realm, mount, true);
    let existing = store.list_webauthn_credentials(&realm.id, &user.id).await?;
    let exclude_credentials: Vec<Value> = existing
        .iter()
        .map(|c| json!({ "type": "public-key", "id": c.credential_id }))
        .collect();
    Ok(json!({
        "rp": {
            "id": rp_id,
            "name": if realm.web_authn_policy_passwordless_rp_entity_name.is_empty() {
                &realm.display_name
            } else {
                &realm.web_authn_policy_passwordless_rp_entity_name
            }
        },
        "user": {
            "id": user_handle,
            "name": user.username,
            "displayName": user.email.as_deref().unwrap_or(&user.username)
        },
        "challenge": challenge_b64,
        "pubKeyCredParams": [
            { "type": "public-key", "alg": COSE_ALG_ES256 },
            { "type": "public-key", "alg": COSE_ALG_EDDSA },
            { "type": "public-key", "alg": COSE_ALG_RS256 }
        ],
        "attestation": "none",
        "authenticatorSelection": {
            "residentKey": "preferred",
            "userVerification": "required"
        },
        "excludeCredentials": exclude_credentials
    }))
}

/// Verifies a registration ceremony response and stores the `WebAuthnCredentialRecord` (W3C WebAuthn L3 §7.1, K11).
pub async fn complete_passkey_registration(
    store: &Arc<dyn Store>,
    mount: &Mount,
    realm: &RealmConfig,
    user: &mut UserRepresentation,
    challenge_b64: &str,
    response_json: &Value,
    label: Option<String>,
    require_uv: bool,
) -> Result<WebAuthnCredentialRecord> {
    let parsed: RegistrationClientResponse = serde_json::from_value(response_json.clone())
        .map_err(|e| {
            KaisatsuError::BadRequest(format!("invalid WebAuthn registration JSON: {e}"))
        })?;
    let rp_id = effective_rp_id(realm, mount, true);
    let origins = allowed_origins_for_realm(realm, mount);
    let verified = verify_registration_response(
        &parsed,
        challenge_b64,
        &rp_id,
        &origins,
        require_uv,
        &[COSE_ALG_ES256, COSE_ALG_EDDSA, COSE_ALG_RS256],
    )?;

    // W3C WebAuthn L3 §7.1 step 26: Verify that the credentialId is not yet registered for any user.
    if store
        .get_webauthn_by_credential_id(&realm.id, &verified.credential_id_b64)
        .await?
        .is_some()
    {
        return Err(KaisatsuError::Conflict(
            "WebAuthn credentialId is already registered in this realm".into(),
        ));
    }

    let user_handle = ensure_user_handle(store, &realm.id, user).await?;
    let cose_json = serde_json::to_string(&verified.public_key)
        .map_err(|e| KaisatsuError::Crypto(e.to_string()))?;

    let rec = WebAuthnCredentialRecord {
        id: uuid::Uuid::now_v7().to_string(),
        realm_id: realm.id.clone(),
        org_id: user.org_id.clone(),
        user_id: user.id.clone(),
        user_handle,
        credential_id: verified.credential_id_b64,
        public_key_cose_json: cose_json,
        cose_alg: verified.public_key.alg(),
        sign_count: verified.sign_count,
        uv_initialized: verified.uv_initialized,
        backup_eligible: verified.backup_eligible,
        backup_state: verified.backup_state,
        transports: verified.transports,
        attestation_type: verified.attestation_type,
        rp_id,
        user_label: label.or_else(|| Some("Passkey".into())),
        created_at: Utc::now(),
        last_used_at: None,
    };
    store.insert_webauthn_credential(rec).await
}

/// Verifies an authentication assertion ceremony and updates the stored credential's counter and
/// backup state (W3C WebAuthn L3 §7.2, K11). Returns `(UserRepresentation, user_verified)`.
pub async fn complete_passkey_authentication(
    store: &Arc<dyn Store>,
    mount: &Mount,
    realm: &RealmConfig,
    expected_user_id: Option<&str>,
    challenge_b64: &str,
    response_json: &Value,
    require_uv: bool,
) -> Result<(UserRepresentation, bool)> {
    let parsed: AuthenticationClientResponse = serde_json::from_value(response_json.clone())
        .map_err(|e| KaisatsuError::BadRequest(format!("invalid WebAuthn assertion JSON: {e}")))?;

    let mut cred = store
        .get_webauthn_by_credential_id(&realm.id, &parsed.id)
        .await?
        .ok_or_else(|| KaisatsuError::Unauthorized("unknown WebAuthn credential".into()))?;

    // W3C WebAuthn L3 §7.2 step 6:
    // - If user was identified beforehand, verify the credential belongs to that user.
    // - If user was not identified beforehand (conditional UI), verify response.userHandle is present.
    if let Some(uid) = expected_user_id {
        if cred.user_id != uid {
            return Err(KaisatsuError::Unauthorized(
                "WebAuthn credential does not belong to the identified user".into(),
            ));
        }
    } else if parsed
        .response
        .user_handle
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return Err(KaisatsuError::Unauthorized(
            "usernameless passkey login requires userHandle".into(),
        ));
    }

    let user = store
        .get_user_by_id(&realm.id, None, &cred.user_id)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::Unauthorized("user not found or disabled".into()))?;

    let pub_key: CosePublicKey = serde_json::from_str(&cred.public_key_cose_json)
        .map_err(|_| KaisatsuError::Crypto("invalid stored COSE public key".into()))?;
    let rp_id = effective_rp_id(realm, mount, true);
    let origins = allowed_origins_for_realm(realm, mount);

    let verified = verify_authentication_response(
        &parsed,
        challenge_b64,
        &rp_id,
        &origins,
        require_uv,
        &cred.credential_id,
        &cred.user_handle,
        &pub_key,
        cred.sign_count,
        cred.backup_eligible,
    )?;

    cred.sign_count = verified.new_sign_count;
    cred.backup_state = verified.backup_state;
    if !cred.uv_initialized && verified.user_verified {
        cred.uv_initialized = true;
    }
    cred.last_used_at = Some(Utc::now());
    store.update_webauthn_credential(cred).await?;

    Ok((user, verified.user_verified))
}
