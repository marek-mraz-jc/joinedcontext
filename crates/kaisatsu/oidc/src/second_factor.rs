//! Whether a person must pass a second factor at sign-in. One check for the login flow, the
//! password grant, the refresh grant and the account console, so no grant asks less than another.

use kaisatsu_core::{RealmConfig, Result, SsoSession, UserRepresentation};
use serde::Deserialize;

use crate::OidcState;

/// The deployment's floor, `[kaisatsu] second_factor`: `optional` keeps the per-person rules,
/// `required` asks every person of every realm. Service accounts (client_credentials,
/// jwt-bearer) never pass through this check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecondFactor {
    #[default]
    Optional,
    Required,
}

/// The attribute that asks a second factor of every member unless it says `optional`: on a kaisatsu
/// Organization, or on a group of the person (the Portal's reconciler writes it on the groups
/// that carry an Organization's `jc_org`).
pub const ORG_SECOND_FACTOR_ATTRIBUTE: &str = "jc.secondFactor";

/// True when `user` must pass a second factor: the deployment floor ([`OidcState::second_factor`]) is `required`, the realm is
/// `master`, the person holds the effective realm role `mfa-required` (direct, composite or by
/// group), or [`ORG_SECOND_FACTOR_ATTRIBUTE`] is `required` on one of the person's groups or on
/// any Organization the person belongs to (membership or group `jc_org`). Strictest wins: nothing here can lower another rule.
pub async fn second_factor_required(
    state: &OidcState,
    realm: &RealmConfig,
    user: &UserRepresentation,
) -> Result<bool> {
    let store = &state.store;
    if state.second_factor == SecondFactor::Required || realm.realm == "master" {
        return Ok(true);
    }
    let user_groups = store.list_user_groups(&realm.id, &user.id).await?;
    let all_roles = store.list_roles(&realm.id, None).await?;
    let group_roles: Vec<String> = user_groups
        .iter()
        .flat_map(|g| g.realm_roles.iter().cloned())
        .collect();
    let eff_roles = kaisatsu_core::claims::resolve_effective_realm_roles(
        &user.realm_roles,
        &group_roles,
        &all_roles,
    );
    // Fails closed: any value other than `optional` (a typo, an empty string) asks the factor.
    let says_required =
        |values: Option<&Vec<String>>| values.is_some_and(|v| v.iter().any(|s| s != "optional"));
    // The Portal's reconciler writes the attribute on the groups that carry `jc_org`.
    if eff_roles.iter().any(|r| r == "mfa-required")
        || user_groups
            .iter()
            .any(|g| says_required(g.attributes.get(ORG_SECOND_FACTOR_ATTRIBUTE)))
    {
        return Ok(true);
    }
    let mut org_ids: Vec<String> = store
        .list_memberships_for_person(&realm.id, &user.id)
        .await?
        .into_iter()
        .map(|m| m.org_id)
        .collect();
    for g in &user_groups {
        org_ids.extend(g.attributes.get("jc_org").into_iter().flatten().cloned());
    }
    org_ids.sort();
    org_ids.dedup();
    for org_id in &org_ids {
        if let Some(org) = store.get_organization(&realm.id, org_id).await?
            && says_required(org.attributes.get(ORG_SECOND_FACTOR_ATTRIBUTE))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// True when an existing SSO session may stand in for a sign-in (silent SSO, `prompt=none`,
/// device approval): it passed a second factor, or its person needs none. A session from
/// before the rule covered the person signs in again instead of minting a code.
pub async fn sso_session_suffices(
    state: &OidcState,
    realm: &RealmConfig,
    sso: &SsoSession,
) -> Result<bool> {
    if sso.acr == "mfa" {
        return Ok(true);
    }
    match state
        .store
        .get_user_by_id(&realm.id, None, &sso.user_id)
        .await?
    {
        Some(user) => Ok(!second_factor_required(state, realm, &user).await?),
        None => Ok(false),
    }
}
