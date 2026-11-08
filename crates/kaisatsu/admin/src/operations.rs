#![forbid(unsafe_code)]
//! One executor for every registry operation (W5). The OData service (entity sets, bound and
//! unbound actions and functions), MCP `tools/call` and the native REST routes all call
//! [`execute`], so the answer and the authorization decision are the same whichever way a caller
//! comes in. Each operation looks its object up inside the caller's organizations first, so a
//! missing id and another organization's id answer the same `404`.

use crate::compat::{
    AdminState, check_decision, ensure_may_map_client_roles, merge_user_update, record_admin_write,
};
use chrono::Utc;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_oidc::dispatch_backchannel_logout;
use odata_table::QueryOptions;
use serde_json::{Value, json};
use std::sync::Arc;

fn str_param<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn bool_param(params: &Value, name: &str) -> Option<bool> {
    params.get(name).and_then(Value::as_bool)
}

fn str_list(params: &Value, name: &str) -> Result<Vec<String>> {
    params
        .get(name)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .ok_or_else(|| KaisatsuError::BadRequest(format!("parameter {name} must be a list")))
}

fn required_key(key: Option<&str>) -> Result<&str> {
    key.filter(|k| !k.is_empty())
        .ok_or_else(|| KaisatsuError::BadRequest("the operation needs an entity key".into()))
}

pub(crate) async fn visible_person(
    state: &AdminState,
    ctx: &RequestCtx,
    id: &str,
) -> Result<UserRepresentation> {
    let allowed = ctx.allowed_orgs();
    // A person may always find themselves (self-service operations); anyone else only inside
    // the caller's organizations.
    let scope = if ctx.principal.subject.as_deref() == Some(id) {
        None
    } else {
        allowed
    };
    state
        .oidc
        .store
        .get_user_by_id(&ctx.realm_id, scope.as_deref(), id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))
}

pub(crate) fn person_target(ctx: &RequestCtx, user: &UserRepresentation) -> ObjectTarget {
    ObjectTarget::Person {
        realm_id: ctx.realm_id.clone(),
        org_id: user.org_id.clone(),
        person_id: user.id.clone(),
    }
}

/// Runs registry operation `op` for `ctx`. `key` is the bound entity's key (or `None`),
/// `params` the action/function parameters (or the entity body of a set write). `Ok(None)` is
/// an operation without a result (OData answers 204).
pub async fn execute(
    state: &AdminState,
    mount: &Mount,
    ctx: &RequestCtx,
    op: &OperationDef,
    key: Option<&str>,
    params: &Value,
) -> Result<Option<Value>> {
    let realm = ctx.realm_id.as_str();
    let store = &state.oidc.store;
    let authz = state.authorizer.as_ref();

    if op.odata.kind == ODataKind::EntitySetRead {
        let set = op.odata.name;
        let mut options = QueryOptions::default();
        if let Some(f) = str_param(params, "filter") {
            options = crate::odata::parse_odata_options(
                set,
                Some(&format!(
                    "$filter={}",
                    url::form_urlencoded::byte_serialize(f.as_bytes()).collect::<String>()
                )),
            )?
            .2;
        }
        let ans = store
            .query_entity_set(ctx, authz, set, &options, 100)
            .await?;
        return Ok(Some(Value::Array(ans.rows)));
    }

    match op.name {
        "person_create" => {
            let mut user: UserRepresentation = serde_json::from_value(params.clone())
                .map_err(|e| KaisatsuError::BadRequest(format!("invalid person: {e}")))?;
            user.realm_id = realm.to_owned();
            if user.id.is_empty() || !ctx.principal.is_realm_admin() {
                user.id = uuid::Uuid::now_v7().to_string();
            }
            if user.org_id.is_none() {
                user.org_id = ctx.active_org_id.clone();
            }
            // Keycloak's create ignores role mappings, groups and service-account binding.
            user.realm_roles.clear();
            user.client_roles.clear();
            user.groups.clear();
            user.service_account_client_id = None;
            user.created_timestamp = Some(Utc::now().timestamp_millis());
            check_decision(authz.check(ctx, Action::Create, &person_target(ctx, &user)))?;
            let created = store.create_user(user).await?;
            record_admin_write(
                state,
                ctx,
                "CREATE",
                "USER",
                &format!("users/{}", created.id),
                serde_json::to_value(&created).ok(),
            )
            .await?;
            Ok(Some(kaisatsu_core::odata_defs::person_to_row(&created)))
        }
        "person_update" => {
            let existing = visible_person(state, ctx, required_key(key)?).await?;
            check_decision(authz.check(ctx, Action::Update, &person_target(ctx, &existing)))?;
            let mut incoming = serde_json::to_value(&existing).unwrap_or(Value::Null);
            if let (Some(base), Some(patch)) = (incoming.as_object_mut(), params.as_object()) {
                for (k, v) in patch {
                    base.insert(k.clone(), v.clone());
                }
            }
            let incoming: UserRepresentation = serde_json::from_value(incoming)
                .map_err(|e| KaisatsuError::BadRequest(format!("invalid person: {e}")))?;
            let merged = merge_user_update(&existing, incoming, ctx);
            let updated = store.update_user(merged).await?;
            record_admin_write(
                state,
                ctx,
                "UPDATE",
                "USER",
                &format!("users/{}", updated.id),
                serde_json::to_value(&updated).ok(),
            )
            .await?;
            Ok(Some(kaisatsu_core::odata_defs::person_to_row(&updated)))
        }
        "person_delete" => {
            let existing = visible_person(state, ctx, required_key(key)?).await?;
            check_decision(authz.check(ctx, Action::Delete, &person_target(ctx, &existing)))?;
            let allowed = ctx.allowed_orgs();
            store
                .delete_user(realm, allowed.as_deref(), &existing.id)
                .await?;
            record_admin_write(
                state,
                ctx,
                "DELETE",
                "USER",
                &format!("users/{}", existing.id),
                None,
            )
            .await?;
            Ok(None)
        }
        "member_add" => {
            let user = visible_person(state, ctx, required_key(key)?).await?;
            let group_id = str_param(params, "groupId")
                .ok_or_else(|| KaisatsuError::BadRequest("groupId is required".into()))?;
            let allowed = ctx.allowed_orgs();
            let group = store
                .get_group(realm, allowed.as_deref(), group_id)
                .await?
                .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
            check_decision(authz.check(
                ctx,
                Action::Update,
                &ObjectTarget::Group {
                    realm_id: realm.to_owned(),
                    org_id: group.org_id.clone(),
                    group_id: group.id.clone(),
                    managed_by: group.managed_by.clone(),
                },
            ))?;
            store.add_user_to_group(realm, &user.id, &group.id).await?;
            record_admin_write(
                state,
                ctx,
                "CREATE",
                "GROUP_MEMBERSHIP",
                &format!("users/{}/groups/{}", user.id, group.id),
                None,
            )
            .await?;
            Ok(None)
        }
        "role_map" => {
            let user = visible_person(state, ctx, required_key(key)?).await?;
            check_decision(authz.check(ctx, Action::Update, &person_target(ctx, &user)))?;
            let client_uuid = str_param(params, "clientUuid")
                .ok_or_else(|| KaisatsuError::BadRequest("clientUuid is required".into()))?;
            ensure_may_map_client_roles(state, ctx, realm, client_uuid).await?;
            for role in str_list(params, "roles")? {
                store
                    .add_role_mapping(RoleMappingRecord {
                        id: uuid::Uuid::now_v7().to_string(),
                        realm_id: realm.to_owned(),
                        org_id: user.org_id.clone(),
                        principal_kind: "user".into(),
                        principal_id: user.id.clone(),
                        role_id: role.clone(),
                        role_name: role,
                        client_uuid: Some(client_uuid.to_owned()),
                    })
                    .await?;
            }
            record_admin_write(
                state,
                ctx,
                "CREATE",
                "CLIENT_ROLE_MAPPING",
                &format!("users/{}/role-mappings/clients/{client_uuid}", user.id),
                None,
            )
            .await?;
            Ok(None)
        }
        "credential_reset" => {
            let user = visible_person(state, ctx, required_key(key)?).await?;
            check_decision(authz.check(ctx, Action::Update, &person_target(ctx, &user)))?;
            let value = str_param(params, "value").unwrap_or_default();
            let temporary = bool_param(params, "temporary").unwrap_or(false);
            let realm_cfg = store.get_realm(realm).await?.unwrap_or_default();
            let history: Vec<CredentialRecord> = store
                .list_credentials(realm, &user.id)
                .await?
                .into_iter()
                .filter(|c| c.credential_type.starts_with("password"))
                .collect();
            kaisatsu_crypto::check_password_policy(
                &realm_cfg.password_policy,
                value,
                &user.username,
                user.email.as_deref(),
                &history,
            )?;
            let phc = kaisatsu_crypto::hash_password(value)?;
            store
                .upsert_credential(CredentialRecord {
                    id: uuid::Uuid::now_v7().to_string(),
                    realm_id: realm.to_owned(),
                    user_id: user.id.clone(),
                    credential_type: "password".into(),
                    user_label: Some("Password".into()),
                    created_date: Utc::now().timestamp_millis(),
                    secret_data: phc,
                    credential_data: String::new(),
                    temporary,
                })
                .await?;
            record_admin_write(
                state,
                ctx,
                "ACTION",
                "USER",
                &format!("users/{}/reset-password", user.id),
                Some(json!({ "temporary": temporary })),
            )
            .await?;
            Ok(None)
        }
        "send_required_actions" => {
            let mut user = visible_person(state, ctx, required_key(key)?).await?;
            check_decision(authz.check(ctx, Action::Update, &person_target(ctx, &user)))?;
            let actions = str_list(params, "actions")?;
            let realm_cfg = store
                .get_realm(realm)
                .await?
                .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
            // Portal `people.rs:365`: without SMTP the call fails, so the Portal falls back to a
            // temporary password (PF-92). The person is not changed in that case.
            if realm_cfg.smtp_server.is_none() {
                return Err(KaisatsuError::NotSupported(
                    "SMTP is not configured on this realm".into(),
                ));
            }
            let client_id = str_param(params, "client_id")
                .or_else(|| str_param(params, "clientId"))
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let redirect_uri = str_param(params, "redirect_uri")
                .or_else(|| str_param(params, "redirectUri"))
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            if let Some(ref r_uri) = redirect_uri {
                let cid = client_id.as_deref().ok_or_else(|| {
                    KaisatsuError::BadRequest(
                        "redirect_uri on execute-actions-email requires client_id".into(),
                    )
                })?;
                let client = store
                    .get_client_by_client_id(realm, cid)
                    .await?
                    .ok_or_else(|| KaisatsuError::BadRequest("unknown client_id".into()))?;
                if !kaisatsu_oidc::validate_redirect_uri(&client.redirect_uris, r_uri) {
                    return Err(KaisatsuError::BadRequest("invalid redirect_uri".into()));
                }
            }
            let lifespan_secs = params
                .get("lifespan")
                .and_then(Value::as_i64)
                .unwrap_or(900)
                .clamp(60, 43_200);
            for a in &actions {
                if !user.required_actions.contains(a) {
                    user.required_actions.push(a.clone());
                }
            }
            let uid = user.id.clone();
            let to_address = user.email.clone().unwrap_or_else(|| user.username.clone());
            let outbox = EmailOutboxRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.to_owned(),
                org_id: user.org_id.clone(),
                user_id: uid.clone(),
                to_address,
                locale: realm_cfg.default_locale.clone(),
                template_kind: "EXECUTE_ACTIONS".into(),
                action_jti: uuid::Uuid::now_v7().to_string(),
                actions: actions.clone(),
                client_id,
                redirect_uri,
                lifespan_secs,
                issuer: mount.realm_issuer(&realm_cfg.realm),
                status: "pending".into(),
                attempts: 0,
                last_error: None,
                next_attempt_at: Utc::now(),
                created_at: Utc::now(),
                sent_at: None,
            };
            // K9, K10: Write the updated user and the outbox row in the same transaction.
            store.update_user_with_outbox(user, outbox).await?;
            record_admin_write(
                state,
                ctx,
                "ACTION",
                "USER",
                &format!("users/{uid}/execute-actions-email"),
                Some(json!({ "actions": actions })),
            )
            .await?;
            Ok(None)
        }
        "session_revoke" => {
            let user = visible_person(state, ctx, required_key(key)?).await?;
            check_decision(authz.check(
                ctx,
                Action::Delete,
                &ObjectTarget::Session {
                    realm_id: realm.to_owned(),
                    org_id: user.org_id.clone(),
                    person_id: user.id.clone(),
                },
            ))?;
            let realm_cfg = store.get_realm(realm).await?.unwrap_or_default();
            let ended = store.delete_user_sso_sessions(realm, &user.id).await?;
            for sso in &ended {
                let _ = dispatch_backchannel_logout(&state.oidc, mount, &realm_cfg, sso).await;
            }
            record_admin_write(
                state,
                ctx,
                "ACTION",
                "USER",
                &format!("users/{}/logout", user.id),
                None,
            )
            .await?;
            Ok(None)
        }
        "keys_rotate" => {
            check_decision(authz.check(
                ctx,
                Action::Update,
                &ObjectTarget::Realm {
                    realm_id: realm.to_owned(),
                },
            ))?;
            let realm_cfg = store
                .get_realm(realm)
                .await?
                .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
            let algorithm = str_param(params, "algorithm").unwrap_or("ES256");
            let immediate = bool_param(params, "immediate").unwrap_or(true);
            let mut existing = store.list_signing_keys(realm).await?;
            let new_key = kaisatsu_crypto::KeyManager::rotate(
                realm,
                &state.oidc.kek,
                &mut existing,
                algorithm,
                realm_cfg.access_token_lifespan,
                immediate,
            )?;
            for k in existing {
                store.upsert_signing_key(k).await?;
            }
            store.upsert_signing_key(new_key.clone()).await?;
            record_admin_write(
                state,
                ctx,
                "ACTION",
                "KEYS",
                "keys/rotate",
                Some(json!({ "kid": new_key.kid, "algorithm": new_key.algorithm })),
            )
            .await?;
            Ok(Some(
                serde_json::to_value(SigningKeyMetadata::from(&new_key)).unwrap_or(Value::Null),
            ))
        }
        "realm_export" => {
            check_decision(authz.check(
                ctx,
                Action::Read,
                &ObjectTarget::Realm {
                    realm_id: realm.to_owned(),
                },
            ))?;
            if !ctx.principal.is_realm_admin() {
                return Err(KaisatsuError::NotFound("realm not found".into()));
            }
            let with_users = bool_param(params, "withUsers").unwrap_or(false);
            let doc = kaisatsu_import::export_realm(&Arc::clone(store), realm, with_users).await?;
            Ok(Some(doc))
        }
        "realm_apply" => {
            check_decision(authz.check(
                ctx,
                Action::Apply,
                &ObjectTarget::Realm {
                    realm_id: realm.to_owned(),
                },
            ))?;
            let mut doc = params
                .get("configuration")
                .cloned()
                .filter(Value::is_object)
                .ok_or_else(|| {
                    KaisatsuError::BadRequest("configuration must be a realm object".into())
                })?;
            // The bound realm is the one applied, whatever the document says.
            doc["realm"] = Value::String(realm.to_owned());
            let opts = kaisatsu_import::ApplyOptions {
                mode: if bool_param(params, "prune").unwrap_or(false) {
                    kaisatsu_import::ApplyMode::Full
                } else {
                    kaisatsu_import::ApplyMode::NoDelete
                },
                dry_run: bool_param(params, "dryRun").unwrap_or(false),
                allow_literal_secrets: false,
                ..Default::default()
            };
            let report = kaisatsu_import::apply_realm_files(
                &Arc::clone(store),
                &[("odata-apply.json".into(), doc)],
                &opts,
            )
            .await?;
            if !opts.dry_run {
                record_admin_write(state, ctx, "ACTION", "REALM", "apply", None).await?;
            }
            Ok(Some(serde_json::to_value(&report).unwrap_or(Value::Null)))
        }
        "consent_revoke" => {
            let consent_id = required_key(key)?;
            let mut opts = QueryOptions::default();
            let service = kaisatsu_core::odata_defs::kaisatsu_odata_service();
            if let Some(info) = service.info(Arc::new(service.model()), "Consents") {
                opts = odata_table::key_options(&info, &Value::String(consent_id.into()), None)?;
            }
            let ans = store
                .query_entity_set(ctx, authz, "Consents", &opts, 1)
                .await?;
            let row = ans
                .rows
                .into_iter()
                .next()
                .ok_or_else(|| KaisatsuError::NotFound("consent not found".into()))?;
            let user_id = row["user_id"].as_str().unwrap_or_default().to_owned();
            let client_id = row["clientId"].as_str().unwrap_or_default().to_owned();
            let user = visible_person(state, ctx, &user_id).await?;
            let action = if ctx.principal.subject.as_deref() == Some(user.id.as_str()) {
                Action::SelfService
            } else {
                Action::Delete
            };
            check_decision(authz.check(ctx, action, &person_target(ctx, &user)))?;
            store.delete_consent(realm, &user.id, &client_id).await?;
            record_admin_write(
                state,
                ctx,
                "DELETE",
                "CONSENT",
                &format!("users/{}/consents/{client_id}", user.id),
                None,
            )
            .await?;
            Ok(None)
        }
        "recovery_codes_generate" => {
            let user = visible_person(state, ctx, required_key(key)?).await?;
            if ctx.principal.subject.as_deref() != Some(user.id.as_str()) {
                return Err(KaisatsuError::NotFound("user not found".into()));
            }
            check_decision(authz.check(ctx, Action::SelfService, &person_target(ctx, &user)))?;
            let (plain, hashes) = kaisatsu_crypto::generate_recovery_codes();
            for old in store
                .list_credentials(realm, &user.id)
                .await?
                .into_iter()
                .filter(|c| c.credential_type == "recovery-authn-codes")
            {
                store.delete_credential(realm, &user.id, &old.id).await?;
            }
            store
                .upsert_credential(CredentialRecord {
                    id: uuid::Uuid::now_v7().to_string(),
                    realm_id: realm.to_owned(),
                    user_id: user.id.clone(),
                    credential_type: "recovery-authn-codes".into(),
                    user_label: Some("Recovery codes".into()),
                    created_date: Utc::now().timestamp_millis(),
                    secret_data: json!({ "hashes": hashes }).to_string(),
                    credential_data: json!({ "remaining": plain.len() }).to_string(),
                    temporary: false,
                })
                .await?;
            Ok(Some(json!(plain)))
        }
        "totp_enrol" => {
            let user = visible_person(state, ctx, required_key(key)?).await?;
            if ctx.principal.subject.as_deref() != Some(user.id.as_str()) {
                return Err(KaisatsuError::NotFound("user not found".into()));
            }
            check_decision(authz.check(ctx, Action::SelfService, &person_target(ctx, &user)))?;
            let realm_cfg = store.get_realm(realm).await?.unwrap_or_default();
            let meta = json!({
                "subType": "totp",
                "digits": realm_cfg.otp_policy_digits,
                "period": realm_cfg.otp_policy_period,
                "algorithm": realm_cfg.otp_policy_algorithm,
                "secretEncoding": "BASE32",
            });
            let (Some(secret), Some(code)) =
                (str_param(params, "secret"), str_param(params, "code"))
            else {
                // Step 1: a fresh secret for the authenticator app; nothing is stored yet.
                return Ok(Some(json!({
                    "secret": kaisatsu_crypto::generate_totp_secret(),
                    "algorithm": realm_cfg.otp_policy_algorithm,
                    "digits": realm_cfg.otp_policy_digits,
                    "period": realm_cfg.otp_policy_period,
                })));
            };
            // Step 2: the app's code proves the secret was taken; only then it is stored.
            if secret.len() < 32 {
                return Err(KaisatsuError::BadRequest(
                    "the secret must be the 160-bit key step 1 returned".into(),
                ));
            }
            let cred = CredentialRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.to_owned(),
                user_id: user.id.clone(),
                credential_type: "otp".into(),
                user_label: Some("Authenticator".into()),
                created_date: Utc::now().timestamp_millis(),
                secret_data: json!({ "value": secret }).to_string(),
                credential_data: meta.to_string(),
                temporary: false,
            };
            let now = u64::try_from(Utc::now().timestamp()).unwrap_or_default();
            if kaisatsu_crypto::verify_totp_code(
                &cred,
                code,
                realm_cfg.otp_policy_look_ahead_window,
                now,
            )?
            .is_none()
            {
                return Err(KaisatsuError::BadRequest("invalid TOTP code".into()));
            }
            store.upsert_credential(cred.clone()).await?;
            Ok(Some(json!({ "credentialId": cred.id })))
        }
        "passkey_register" => {
            let mut user = visible_person(state, ctx, required_key(key)?).await?;
            if ctx.principal.subject.as_deref() != Some(user.id.as_str())
                && !ctx.principal.is_realm_admin()
            {
                return Err(KaisatsuError::NotFound("user not found".into()));
            }
            check_decision(authz.check(ctx, Action::SelfService, &person_target(ctx, &user)))?;
            let realm_cfg = store.get_realm(realm).await?.unwrap_or_default();
            let Some(cred_json) = params.get("credential").filter(|v| v.is_object()) else {
                // Step 1: issue a single-use challenge and creation options. The challenge is
                // MAC'd for this realm and person, so step 2 accepts only one we issued.
                let challenge = kaisatsu_crypto::issue_registration_challenge(
                    &state.oidc.kek,
                    realm,
                    &user.id,
                    Utc::now().timestamp(),
                );
                let rp_id = if !realm_cfg
                    .web_authn_policy_passwordless_rp_id
                    .trim()
                    .is_empty()
                {
                    realm_cfg
                        .web_authn_policy_passwordless_rp_id
                        .trim()
                        .to_ascii_lowercase()
                } else {
                    mount
                        .host
                        .split_once(':')
                        .map_or(mount.host.as_str(), |(h, _)| h)
                        .to_ascii_lowercase()
                };
                return Ok(Some(json!({
                    "challenge": challenge,
                    "rp": { "id": rp_id, "name": realm_cfg.display_name },
                    "pubKeyCredParams": [
                        { "type": "public-key", "alg": kaisatsu_crypto::COSE_ALG_ES256 },
                        { "type": "public-key", "alg": kaisatsu_crypto::COSE_ALG_EDDSA },
                        { "type": "public-key", "alg": kaisatsu_crypto::COSE_ALG_RS256 }
                    ]
                })));
            };
            let challenge = str_param(params, "challenge").unwrap_or_default();
            if !kaisatsu_crypto::verify_registration_challenge(
                &state.oidc.kek,
                realm,
                &user.id,
                challenge,
                Utc::now().timestamp(),
            ) {
                return Err(KaisatsuError::BadRequest(
                    "unknown or expired WebAuthn challenge".into(),
                ));
            }
            // Enforce single-use challenge via `revoked_token`.
            store
                .mark_token_revoked(RevokedTokenRecord {
                    jti_hash: kaisatsu_crypto::sha256_hex(&format!(
                        "webauthn-chal:{}:{}",
                        user.id, challenge
                    )),
                    realm_id: realm.to_owned(),
                    expires_at: Utc::now() + chrono::Duration::minutes(15),
                })
                .await
                .map_err(|_| KaisatsuError::BadRequest("WebAuthn challenge already used".into()))?;

            let parsed: kaisatsu_crypto::RegistrationClientResponse =
                serde_json::from_value(cred_json.clone()).map_err(|e| {
                    KaisatsuError::BadRequest(format!("invalid WebAuthn credential: {e}"))
                })?;
            let rp_id = if !realm_cfg
                .web_authn_policy_passwordless_rp_id
                .trim()
                .is_empty()
            {
                realm_cfg
                    .web_authn_policy_passwordless_rp_id
                    .trim()
                    .to_ascii_lowercase()
            } else {
                mount
                    .host
                    .split_once(':')
                    .map_or(mount.host.as_str(), |(h, _)| h)
                    .to_ascii_lowercase()
            };
            let origins = vec![
                format!("{}://{}", mount.scheme, mount.host),
                format!("https://{rp_id}"),
            ];
            let verified = kaisatsu_crypto::verify_registration_response(
                &parsed,
                challenge,
                &rp_id,
                &origins,
                false,
                &[
                    kaisatsu_crypto::COSE_ALG_ES256,
                    kaisatsu_crypto::COSE_ALG_EDDSA,
                    kaisatsu_crypto::COSE_ALG_RS256,
                ],
            )?;
            if store
                .get_webauthn_by_credential_id(realm, &verified.credential_id_b64)
                .await?
                .is_some()
            {
                return Err(KaisatsuError::Conflict(
                    "WebAuthn credentialId is already registered in this realm".into(),
                ));
            }
            let user_handle = match user
                .attributes
                .get("webauthn.user.handle")
                .and_then(|v| v.first())
                .cloned()
            {
                Some(h) if !h.is_empty() => h,
                _ => {
                    let h = kaisatsu_crypto::random_handle_256();
                    user.attributes
                        .insert("webauthn.user.handle".into(), vec![h.clone()]);
                    store.update_user(user.clone()).await?;
                    h
                }
            };
            let rec = WebAuthnCredentialRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.to_owned(),
                org_id: user.org_id.clone(),
                user_id: user.id.clone(),
                user_handle,
                credential_id: verified.credential_id_b64,
                public_key_cose_json: serde_json::to_string(&verified.public_key)
                    .unwrap_or_default(),
                cose_alg: verified.public_key.alg(),
                sign_count: verified.sign_count,
                uv_initialized: verified.uv_initialized,
                backup_eligible: verified.backup_eligible,
                backup_state: verified.backup_state,
                transports: verified.transports,
                attestation_type: verified.attestation_type,
                rp_id,
                user_label: str_param(params, "label")
                    .map(str::to_owned)
                    .or_else(|| Some("Passkey".into())),
                created_at: Utc::now(),
                last_used_at: None,
            };
            let inserted = store.insert_webauthn_credential(rec).await?;
            Ok(Some(kaisatsu_core::odata_defs::webauthn_credential_to_row(
                &inserted,
            )))
        }
        "passkey_delete" => {
            let cred_id = required_key(key)?;
            // A person deletes their own passkeys; anyone else needs the right to update people
            // of the organization. A passkey outside both answers 404 like a missing one.
            let own = match ctx.principal.subject.as_deref() {
                Some(sub) => store
                    .list_webauthn_credentials(realm, sub)
                    .await?
                    .iter()
                    .any(|c| c.id == cred_id),
                None => false,
            };
            if !own {
                let decision = authz.check(
                    ctx,
                    Action::Update,
                    &ObjectTarget::Person {
                        realm_id: realm.to_owned(),
                        org_id: ctx.active_org_id.clone(),
                        person_id: "*".into(),
                    },
                );
                if !decision.allowed {
                    return Err(KaisatsuError::NotFound("passkey not found".into()));
                }
            }
            // One's own passkey is found without the organization scope, as `visible_person` does.
            let allowed = if own { None } else { ctx.allowed_orgs() };
            let deleted = store
                .delete_webauthn_credential(realm, allowed.as_deref(), cred_id)
                .await?;
            if !deleted {
                return Err(KaisatsuError::NotFound("passkey not found".into()));
            }
            Ok(None)
        }
        "outbox_retry" => {
            let id = required_key(key)?;
            check_decision(authz.check(
                ctx,
                Action::Update,
                &ObjectTarget::Person {
                    realm_id: realm.to_owned(),
                    org_id: ctx.active_org_id.clone(),
                    person_id: "*".into(),
                },
            ))?;
            let allowed = ctx.allowed_orgs();
            let row = store
                .retry_outbox_row(realm, allowed.as_deref(), id)
                .await?
                .ok_or_else(|| KaisatsuError::NotFound("outbox item not found".into()))?;
            Ok(Some(kaisatsu_core::odata_defs::email_outbox_to_row(&row)))
        }
        "domain_request_verification" => {
            let id = required_key(key)?;
            let allowed = ctx.allowed_orgs();
            let (org, _) =
                crate::domain::find_visible_domain(store, realm, allowed.as_deref(), id).await?;
            check_decision(authz.check(
                ctx,
                Action::Update,
                &ObjectTarget::Organization {
                    realm_id: realm.to_owned(),
                    org_id: org.id,
                },
            ))?;
            let res =
                crate::domain::request_domain_verification(store, realm, allowed.as_deref(), id)
                    .await?;
            record_admin_write(
                state,
                ctx,
                "ACTION",
                "ORGANIZATION_DOMAIN",
                &format!("domains/{id}/request-verification"),
                None,
            )
            .await?;
            Ok(Some(res))
        }
        "domain_verify" => {
            let id = required_key(key)?;
            let allowed = ctx.allowed_orgs();
            let (org, _) =
                crate::domain::find_visible_domain(store, realm, allowed.as_deref(), id).await?;
            check_decision(authz.check(
                ctx,
                Action::Update,
                &ObjectTarget::Organization {
                    realm_id: realm.to_owned(),
                    org_id: org.id,
                },
            ))?;
            let res = crate::domain::verify_organization_domain(
                store,
                state.dns_resolver.as_ref(),
                realm,
                allowed.as_deref(),
                id,
            )
            .await?;
            record_admin_write(
                state,
                ctx,
                "ACTION",
                "ORGANIZATION_DOMAIN",
                &format!("domains/{id}/verify"),
                Some(res.clone()),
            )
            .await?;
            Ok(Some(res))
        }
        "allowed" => allowed(state, ctx, params).await.map(Some),
        other => Err(KaisatsuError::NotSupported(format!(
            "operation {other} has no executor"
        ))),
    }
}

/// `Allowed(operation, object)`: the decision the call itself would make (on the object's own
/// organization, as the call judges it), without revealing a
/// hidden record (a missing id and another organization's id answer the same, W5). `operation`
/// is a registry operation name or an action verb (`read`, `create`, `update`, `delete`,
/// `apply`); `object` is `Set('key')`, a bare set, or absent for the realm.
async fn allowed(state: &AdminState, ctx: &RequestCtx, params: &Value) -> Result<Value> {
    let realm = ctx.realm_id.clone();
    let op_name = str_param(params, "operation").unwrap_or("read");
    let action = match state.registry.find(op_name) {
        Some(op) => op.action,
        None => match op_name {
            "read" => Action::Read,
            "create" => Action::Create,
            "update" => Action::Update,
            "delete" => Action::Delete,
            "apply" => Action::Apply,
            "self" => Action::SelfService,
            other => {
                return Err(KaisatsuError::BadRequest(format!(
                    "unknown operation: {other}"
                )));
            }
        },
    };
    let object = str_param(params, "object").unwrap_or_default();
    let (set, key) = match object.split_once('(') {
        Some((s, rest)) => (
            s,
            Some(
                rest.trim_end_matches(')')
                    .trim_matches('\'')
                    .replace("''", "'"),
            ),
        ),
        None => (object, None),
    };
    let allowed_orgs = ctx.allowed_orgs();
    let store = &state.oidc.store;
    let hidden = json!({ "allowed": false, "reason": "resource not found", "notFound": true });
    let target = match (set, key.as_deref()) {
        ("People", Some(uid)) => {
            let Some(u) = store
                .get_user_by_id(&realm, allowed_orgs.as_deref(), uid)
                .await?
            else {
                return Ok(hidden);
            };
            ObjectTarget::Person {
                realm_id: realm,
                org_id: u.org_id,
                person_id: uid.to_owned(),
            }
        }
        ("Groups", Some(gid)) => {
            let Some(g) = store
                .get_group(&realm, allowed_orgs.as_deref(), gid)
                .await?
            else {
                return Ok(hidden);
            };
            ObjectTarget::Group {
                realm_id: realm,
                org_id: g.org_id,
                group_id: gid.to_owned(),
                managed_by: g.managed_by,
            }
        }
        ("Applications", Some(uuid)) => {
            let Some(c) = store
                .get_client_by_uuid(&realm, allowed_orgs.as_deref(), uuid)
                .await?
            else {
                return Ok(hidden);
            };
            ObjectTarget::Application {
                realm_id: realm,
                org_id: c.org_id,
                client_id: c.client_id,
                managed_by: c.managed_by,
            }
        }
        ("" | "Realms", _) => ObjectTarget::Realm { realm_id: realm },
        (other, _) => {
            return Err(KaisatsuError::BadRequest(format!(
                "Allowed does not judge objects of {other}"
            )));
        }
    };
    let dec = state.authorizer.check(ctx, action, &target);
    if !dec.visible {
        return Ok(hidden);
    }
    Ok(json!({ "allowed": dec.allowed, "reason": dec.reason, "notFound": false }))
}
