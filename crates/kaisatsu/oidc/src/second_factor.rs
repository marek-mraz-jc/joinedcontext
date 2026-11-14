//! Whether a person must pass a second factor at sign-in. One check for the login flow, the
//! password grant, the refresh grant and the account console, so no grant asks less than another.

use kaisatsu_core::{RealmConfig, Result, UserRepresentation};
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

/// The Organization attribute the Portal's reconciler writes: `required` asks every member.
pub const ORG_SECOND_FACTOR_ATTRIBUTE: &str = "jc.secondFactor";

/// True when `user` must pass a second factor: the deployment floor ([`OidcState::second_factor`]) is `required`, the realm is
/// `master`, the person holds the effective realm role `mfa-required` (direct, composite or by
/// group), or any Organization the person belongs to (membership or group `jc_org`) has
/// [`ORG_SECOND_FACTOR_ATTRIBUTE`] `required`. Strictest wins: nothing here can lower another rule.
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
    if eff_roles.iter().any(|r| r == "mfa-required") {
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
            && org
                .attributes
                .get(ORG_SECOND_FACTOR_ATTRIBUTE)
                .is_some_and(|v| v.iter().any(|s| s == "required"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
