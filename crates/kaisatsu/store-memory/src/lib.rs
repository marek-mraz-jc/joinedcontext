#![forbid(unsafe_code)]
//! In-memory implementation of [`kaisatsu_core::Store`] with `odata-table::MemoryTable` backing
//! every entity set and enforcing the same organization isolation as PostgreSQL RLS (W2, W4).

use jc_time::TimestampTz;
use jiff::SignedDuration;
use kaisatsu_core::store::StoreFuture;
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, KeyManager};
use odata_table::{Answer, MemoryTable, QueryOptions, TableInfo, query};
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

fn attach_children(parent: &mut GroupRepresentation, all: &[GroupRepresentation]) {
    let mut children: Vec<GroupRepresentation> = all
        .iter()
        .filter(|c| {
            c.parent_id.as_deref() == Some(parent.id.as_str())
                || (c.parent_id.is_none()
                    && c.path.starts_with(&format!("{}/", parent.path))
                    && !c.path[parent.path.len() + 1..].contains('/'))
        })
        .cloned()
        .collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));
    for ch in &mut children {
        attach_children(ch, all);
    }
    parent.sub_groups = children;
}

/// A page of the group's members, in the order they joined the realm.
fn members_of(
    st: &State,
    group: &GroupRepresentation,
    first: usize,
    max: usize,
) -> Vec<UserRepresentation> {
    let member_ids: Vec<&str> = st
        .group_members
        .iter()
        .filter(|gm| gm.realm_id == group.realm_id && gm.group_id == group.id)
        .map(|gm| gm.user_id.as_str())
        .collect();
    st.users
        .iter()
        .filter(|u| u.realm_id == group.realm_id && member_ids.contains(&u.id.as_str()))
        .skip(first)
        .take(max)
        .cloned()
        .collect()
}

/// Drops what belongs to a person who is gone.
fn drop_person_rows(st: &mut State, realm_id: &str, user_id: &str) {
    st.credentials
        .retain(|c| !(c.realm_id == realm_id && c.user_id == user_id));
    st.group_members
        .retain(|gm| !(gm.realm_id == realm_id && gm.user_id == user_id));
    st.sso_sessions
        .retain(|s| !(s.realm_id == realm_id && s.user_id == user_id));
    st.webauthn_credentials
        .retain(|w| !(w.realm_id == realm_id && w.user_id == user_id));
    st.idp_links
        .retain(|l| !(l.realm_id == realm_id && l.user_id == user_id));
    st.role_mappings.retain(|rm| {
        !(rm.realm_id == realm_id && rm.principal_kind == "user" && rm.principal_id == user_id)
    });
}

/// Every place a document grants client roles in the realm: people, groups and composites.
fn client_grants<'s>(
    st: &'s mut State,
    realm_id: &'s str,
) -> impl Iterator<Item = &'s mut HashMap<String, Vec<String>>> {
    let people = st.users.iter_mut().filter(move |u| u.realm_id == realm_id);
    let groups = st.groups.iter_mut().filter(move |g| g.realm_id == realm_id);
    let roles = st.roles.iter_mut().filter(move |r| r.realm_id == realm_id);
    people
        .map(|u| &mut u.client_roles)
        .chain(groups.map(|g| &mut g.client_roles))
        .chain(roles.map(|r| &mut r.composites.client))
}

#[derive(Default)]
struct State {
    realms: HashMap<String, RealmConfig>,
    organizations: Vec<Organization>,
    memberships: Vec<Membership>,
    users: Vec<UserRepresentation>,
    credentials: Vec<CredentialRecord>,
    groups: Vec<GroupRepresentation>,
    group_members: Vec<GroupMemberRecord>,
    roles: Vec<RoleRepresentation>,
    role_mappings: Vec<RoleMappingRecord>,
    clients: Vec<ClientRepresentation>,
    client_scopes: Vec<ClientScopeRepresentation>,
    idps: Vec<IdentityProviderRepresentation>,
    signing_keys: Vec<SigningKeyRecord>,
    auth_sessions: Vec<AuthSession>,
    auth_codes: Vec<AuthorizationCodeRecord>,
    device_codes: Vec<DeviceCodeRecord>,
    par_requests: Vec<ParRequestRecord>,
    sso_sessions: Vec<SsoSession>,
    client_sessions: Vec<ClientSession>,
    offline_sessions: Vec<OfflineSession>,
    refresh_families: Vec<RefreshTokenFamily>,
    revoked_tokens: Vec<RevokedTokenRecord>,
    login_failures: Vec<LoginFailureRecord>,
    consents: Vec<ConsentRecord>,
    required_actions: Vec<RequiredActionConfig>,
    brandings: Vec<BrandingBlock>,
    events: Vec<AuditEvent>,
    admin_events: Vec<AdminEvent>,
    webauthn_credentials: Vec<WebAuthnCredentialRecord>,
    email_outbox: Vec<EmailOutboxRecord>,
    idp_links: Vec<IdentityProviderLinkRecord>,
}

#[derive(Clone)]
pub struct MemoryStore {
    inner: Arc<RwLock<State>>,
    service: Service,
    csdl_model: Arc<odata_csdl::Model>,
    kek: Kek,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new(Kek::generate_for_tests())
    }
}

impl MemoryStore {
    #[must_use]
    pub fn new(kek: Kek) -> Self {
        let service = kaisatsu_odata_service();
        let csdl_model = Arc::new(service.model());
        Self {
            inner: Arc::new(RwLock::new(State::default())),
            service,
            csdl_model,
            kek,
        }
    }

    pub fn kek(&self) -> &Kek {
        &self.kek
    }

    /// Ensures a realm has active ES256 and RS256 signing keys.
    pub fn ensure_realm_keys(&self, realm_id: &str) -> Result<()> {
        self.ensure_key(realm_id, "ES256")?;
        self.ensure_key(realm_id, "RS256")
    }

    /// Ensures a realm has an active key of `algorithm`, generating one when it has none.
    fn ensure_key(&self, realm_id: &str, algorithm: &str) -> Result<()> {
        let mut st = self.inner.write();
        let has = st
            .signing_keys
            .iter()
            .any(|k| k.realm_id == realm_id && k.algorithm == algorithm && k.status == "ACTIVE");
        if !has {
            let key = KeyManager::generate(algorithm, realm_id, &self.kek, "ACTIVE")?;
            st.signing_keys.push(key);
        }
        Ok(())
    }

    fn org_allowed(allowed_orgs: Option<&[String]>, row_org: Option<&str>) -> bool {
        let Some(allowed) = allowed_orgs else {
            return true;
        };
        let Some(org) = row_org else {
            return false;
        };
        allowed.iter().any(|a| a == org)
    }

    fn user_visible(
        st: &State,
        realm_id: &str,
        allowed_orgs: Option<&[String]>,
        user: &UserRepresentation,
    ) -> bool {
        if user.realm_id != realm_id || user.transient {
            return false;
        }
        let Some(allowed) = allowed_orgs else {
            return true;
        };
        if let Some(org) = &user.org_id
            && allowed.contains(org)
        {
            return true;
        }
        // Check memberships or group paths under `/{org}/...`
        if st.memberships.iter().any(|m| {
            m.realm_id == realm_id && m.person_id == user.id && allowed.contains(&m.org_id)
        }) {
            return true;
        }
        st.group_members
            .iter()
            .filter(|gm| gm.realm_id == realm_id && gm.user_id == user.id)
            .filter_map(|gm| {
                st.groups
                    .iter()
                    .find(|g| g.realm_id == realm_id && g.id == gm.group_id)
            })
            .any(|g| {
                g.org_id.as_ref().is_some_and(|o| allowed.contains(o))
                    || g.path
                        .strip_prefix('/')
                        .and_then(|p| p.split('/').next())
                        .is_some_and(|root| allowed.iter().any(|a| a == root))
            })
    }

    fn build_entity_rows(&self, st: &State, realm_id: &str, set_name: &str) -> Vec<Value> {
        match set_name {
            "Realms" => st
                .realms
                .values()
                .filter(|r| r.id == realm_id || r.realm == realm_id)
                .map(odata_defs::realm_to_row)
                .collect(),
            "Organizations" => st
                .organizations
                .iter()
                .filter(|o| o.realm_id == realm_id)
                .map(odata_defs::org_to_row)
                .collect(),
            "OrganizationDomains" => st
                .organizations
                .iter()
                .filter(|o| o.realm_id == realm_id)
                .flat_map(|o| o.domains.iter())
                .map(odata_defs::org_domain_to_row)
                .collect(),
            "People" => st
                .users
                .iter()
                .filter(|u| u.realm_id == realm_id && !u.transient)
                .map(odata_defs::person_to_row)
                .collect(),
            "Memberships" => st
                .memberships
                .iter()
                .filter(|m| m.realm_id == realm_id)
                .map(odata_defs::membership_to_row)
                .collect(),
            "Groups" => st
                .groups
                .iter()
                .filter(|g| g.realm_id == realm_id)
                .map(odata_defs::group_to_row)
                .collect(),
            "GroupMembers" => st
                .group_members
                .iter()
                .filter(|m| m.realm_id == realm_id)
                .map(odata_defs::group_member_to_row)
                .collect(),
            "Roles" => st
                .roles
                .iter()
                .filter(|r| r.realm_id == realm_id)
                .map(odata_defs::role_to_row)
                .collect(),
            "RoleMappings" => st
                .role_mappings
                .iter()
                .filter(|rm| rm.realm_id == realm_id)
                .map(odata_defs::role_mapping_to_row)
                .collect(),
            "Applications" => st
                .clients
                .iter()
                .filter(|c| c.realm_id == realm_id)
                .map(odata_defs::client_to_row)
                .collect(),
            "ClientScopes" => st
                .client_scopes
                .iter()
                .filter(|s| s.realm_id == realm_id)
                .map(odata_defs::scope_to_row)
                .collect(),
            "ProtocolMappers" => st
                .clients
                .iter()
                .filter(|c| c.realm_id == realm_id)
                .flat_map(|c| c.protocol_mappers.iter())
                .chain(
                    st.client_scopes
                        .iter()
                        .filter(|s| s.realm_id == realm_id)
                        .flat_map(|s| s.protocol_mappers.iter()),
                )
                .map(odata_defs::mapper_to_row)
                .collect(),
            "IdentityProviders" => st
                .idps
                .iter()
                .filter(|i| i.realm_id == realm_id)
                .map(odata_defs::idp_to_row)
                .collect(),
            "Credentials" => st
                .credentials
                .iter()
                .filter(|c| c.realm_id == realm_id)
                .map(|c| {
                    let u_org = st
                        .users
                        .iter()
                        .find(|u| u.realm_id == c.realm_id && u.id == c.user_id)
                        .and_then(|u| u.org_id.as_deref());
                    odata_defs::credential_to_row(c, u_org)
                })
                .collect(),
            "Sessions" => st
                .sso_sessions
                .iter()
                .filter(|s| s.realm_id == realm_id)
                .map(odata_defs::session_to_row)
                .collect(),
            "ClientSessions" => st
                .client_sessions
                .iter()
                .filter(|cs| cs.realm_id == realm_id)
                .map(odata_defs::client_session_to_row)
                .collect(),
            "OfflineSessions" => st
                .offline_sessions
                .iter()
                .filter(|os| os.realm_id == realm_id)
                .map(odata_defs::offline_session_to_row)
                .collect(),
            "Consents" => st
                .consents
                .iter()
                .filter(|c| c.realm_id == realm_id)
                .map(odata_defs::consent_to_row)
                .collect(),
            "Events" => st
                .events
                .iter()
                .filter(|e| e.realm_id == realm_id)
                .map(odata_defs::event_to_row)
                .collect(),
            "AdminEvents" => st
                .admin_events
                .iter()
                .filter(|ae| ae.realm_id == realm_id)
                .map(odata_defs::admin_event_to_row)
                .collect(),
            "SigningKeys" => st
                .signing_keys
                .iter()
                .filter(|k| k.realm_id == realm_id)
                .map(odata_defs::signing_key_to_row)
                .collect(),
            "RequiredActions" => st
                .required_actions
                .iter()
                .filter(|ra| ra.realm_id == realm_id)
                .map(odata_defs::required_action_to_row)
                .collect(),
            "BrandingBlocks" => st
                .brandings
                .iter()
                .filter(|b| b.realm_id == realm_id)
                .map(odata_defs::branding_to_row)
                .collect(),
            "WebAuthnCredentials" => st
                .webauthn_credentials
                .iter()
                .filter(|w| w.realm_id == realm_id)
                .map(odata_defs::webauthn_credential_to_row)
                .collect(),
            "EmailOutbox" => st
                .email_outbox
                .iter()
                .filter(|e| e.realm_id == realm_id)
                .map(odata_defs::email_outbox_to_row)
                .collect(),
            "IdentityProviderLinks" => st
                .idp_links
                .iter()
                .filter(|l| l.realm_id == realm_id)
                .map(odata_defs::idp_link_to_row)
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn table_info(&self, set_name: &str) -> Option<TableInfo> {
        self.service.info(self.csdl_model.clone(), set_name)
    }
}

use odata_table::Service;

impl Store for MemoryStore {
    fn query_entity_set<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        authorizer: &'a dyn Authorizer,
        set_name: &'a str,
        options: &'a QueryOptions,
        max_page: u64,
    ) -> StoreFuture<'a, Answer> {
        Box::pin(async move {
            let info = self
                .table_info(set_name)
                .ok_or_else(|| KaisatsuError::NotFound(format!("unknown entity set {set_name}")))?;
            let scope = scope_for_context(ctx, authorizer, set_name);
            if !scope.holds("read") {
                return Err(KaisatsuError::NotFound(format!(
                    "{set_name} not accessible"
                )));
            }
            let rows = {
                let st = self.inner.read();
                self.build_entity_rows(&st, &ctx.realm_id, set_name)
            };
            let table = MemoryTable::new(info, rows)?;
            let ans = query(&table, options, &scope, max_page).await?;
            Ok(ans)
        })
    }

    fn get_realm<'a>(&'a self, realm_name: &'a str) -> StoreFuture<'a, Option<RealmConfig>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .realms
                .get(realm_name)
                .cloned()
                .or_else(|| st.realms.values().find(|r| r.realm == realm_name).cloned()))
        })
    }

    fn upsert_realm<'a>(&'a self, mut realm: RealmConfig) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if realm.id.is_empty() {
                realm.id = realm.realm.clone();
            }
            let realm_id = realm.id.clone();
            {
                let mut st = self.inner.write();
                st.realms.insert(realm.realm.clone(), realm);
            }
            self.ensure_realm_keys(&realm_id)?;
            Ok(())
        })
    }

    fn list_realms<'a>(&'a self) -> StoreFuture<'a, Vec<RealmConfig>> {
        Box::pin(async move {
            let st = self.inner.read();
            let mut list: Vec<_> = st.realms.values().cloned().collect();
            list.sort_by(|a, b| a.realm.cmp(&b.realm));
            Ok(list)
        })
    }

    fn upsert_organization<'a>(&'a self, mut org: Organization) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if org.id.is_empty() {
                org.id = org.alias.clone();
            }
            let mut st = self.inner.write();
            st.organizations.retain(|o| {
                !(o.realm_id == org.realm_id && (o.id == org.id || o.alias == org.alias))
            });
            st.organizations.push(org);
            Ok(())
        })
    }

    fn get_organization<'a>(
        &'a self,
        realm_id: &'a str,
        org_id_or_alias: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .organizations
                .iter()
                .find(|o| {
                    o.realm_id == realm_id
                        && (o.id == org_id_or_alias || o.alias == org_id_or_alias)
                })
                .cloned())
        })
    }

    fn list_organizations<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<Organization>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .organizations
                .iter()
                .filter(|o| {
                    o.realm_id == realm_id
                        && (Self::org_allowed(allowed_orgs, Some(&o.id))
                            || Self::org_allowed(allowed_orgs, Some(&o.alias)))
                })
                .cloned()
                .collect())
        })
    }

    fn find_org_by_domain<'a>(
        &'a self,
        realm_id: &'a str,
        domain: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        Box::pin(async move {
            let st = self.inner.read();
            let lower = domain.to_ascii_lowercase();
            Ok(st
                .organizations
                .iter()
                .find(|o| {
                    o.realm_id == realm_id
                        && o.enabled
                        && o.domains
                            .iter()
                            .any(|d| d.verified && d.name.eq_ignore_ascii_case(&lower))
                })
                .cloned())
        })
    }

    fn upsert_membership<'a>(&'a self, m: Membership) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.memberships.retain(|x| {
                !(x.realm_id == m.realm_id && x.org_id == m.org_id && x.person_id == m.person_id)
            });
            st.memberships.push(m);
            Ok(())
        })
    }

    fn list_memberships_for_person<'a>(
        &'a self,
        realm_id: &'a str,
        person_id: &'a str,
    ) -> StoreFuture<'a, Vec<Membership>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .memberships
                .iter()
                .filter(|m| m.realm_id == realm_id && m.person_id == person_id)
                .cloned()
                .collect())
        })
    }

    fn create_user<'a>(
        &'a self,
        mut user: UserRepresentation,
    ) -> StoreFuture<'a, UserRepresentation> {
        Box::pin(async move {
            if user.id.is_empty() {
                user.id = uuid::Uuid::now_v7().to_string();
            }
            user.username = user.username.to_ascii_lowercase();
            if let Some(em) = &mut user.email {
                *em = em.to_ascii_lowercase();
            }
            // Membership is a row: the document never carries a group path.
            user.groups.clear();
            if user.created_timestamp.is_none() {
                user.created_timestamp = Some(TimestampTz::now().as_millis());
            }
            let mut st = self.inner.write();
            let dup = st.users.iter().any(|u| {
                u.realm_id == user.realm_id
                    && (u.username.eq_ignore_ascii_case(&user.username)
                        || (user.email.is_some()
                            && u.email
                                .as_deref()
                                .is_some_and(|e| Some(e) == user.email.as_deref())))
            });
            if dup {
                return Err(KaisatsuError::Conflict(
                    "user with same username or email already exists".into(),
                ));
            }
            st.users.push(user.clone());
            Ok(user)
        })
    }

    fn update_user<'a>(
        &'a self,
        mut user: UserRepresentation,
    ) -> StoreFuture<'a, UserRepresentation> {
        Box::pin(async move {
            user.username = user.username.to_ascii_lowercase();
            if let Some(em) = &mut user.email {
                *em = em.to_ascii_lowercase();
            }
            user.groups.clear();
            let mut st = self.inner.write();
            let pos = st
                .users
                .iter()
                .position(|u| u.realm_id == user.realm_id && u.id == user.id)
                .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
            let dup = st.users.iter().enumerate().any(|(idx, u)| {
                idx != pos
                    && u.realm_id == user.realm_id
                    && (u.username.eq_ignore_ascii_case(&user.username)
                        || (user.email.is_some()
                            && u.email
                                .as_deref()
                                .is_some_and(|e| Some(e) == user.email.as_deref())))
            });
            if dup {
                return Err(KaisatsuError::Conflict(
                    "another user already holds that username or email".into(),
                ));
            }
            user.realm_roles.clone_from(&st.users[pos].realm_roles);
            user.client_roles.clone_from(&st.users[pos].client_roles);
            st.users[pos] = user.clone();
            Ok(user)
        })
    }

    fn set_user_grants<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        realm_roles: &'a [String],
        client_roles: &'a HashMap<String, Vec<String>>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let user = st
                .users
                .iter_mut()
                .find(|u| u.realm_id == realm_id && u.id == user_id)
                .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
            user.realm_roles = realm_roles.to_vec();
            user.client_roles.clone_from(client_roles);
            Ok(())
        })
    }

    fn delete_user<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(pos) = st
                .users
                .iter()
                .position(|u| u.realm_id == realm_id && u.id == user_id)
            else {
                return Ok(false);
            };
            if !Self::user_visible(&st, realm_id, allowed_orgs, &st.users[pos]) {
                return Ok(false);
            }
            st.users.remove(pos);
            drop_person_rows(&mut st, realm_id, user_id);
            Ok(true)
        })
    }

    fn get_user_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let Some(user) = st
                .users
                .iter()
                .find(|u| u.realm_id == realm_id && u.id == user_id)
            else {
                return Ok(None);
            };
            if !Self::user_visible(&st, realm_id, allowed_orgs, user) {
                return Ok(None);
            }
            Ok(Some(user.clone()))
        })
    }

    fn get_user_by_username_or_email<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        identifier: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let lower = identifier.trim().to_ascii_lowercase();
            let Some(user) = st.users.iter().find(|u| {
                u.realm_id == realm_id
                    && (u.username.eq_ignore_ascii_case(&lower)
                        || u.email
                            .as_deref()
                            .is_some_and(|e| e.eq_ignore_ascii_case(&lower)))
            }) else {
                return Ok(None);
            };
            if !Self::user_visible(&st, realm_id, allowed_orgs, user) {
                return Ok(None);
            }
            Ok(Some(user.clone()))
        })
    }

    fn list_users<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        email: Option<&'a str>,
        username: Option<&'a str>,
        exact: bool,
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let search_lower = search.map(|s| s.to_ascii_lowercase());
            let email_lower = email.map(|s| s.to_ascii_lowercase());
            let user_lower = username.map(|s| s.to_ascii_lowercase());

            let mut out: Vec<UserRepresentation> = st
                .users
                .iter()
                .filter(|u| Self::user_visible(&st, realm_id, allowed_orgs, u))
                .filter(|u| {
                    if let Some(em) = &email_lower {
                        let u_em = u.email.as_deref().unwrap_or_default().to_ascii_lowercase();
                        if exact {
                            if &u_em != em {
                                return false;
                            }
                        } else if !u_em.contains(em) {
                            return false;
                        }
                    }
                    if let Some(un) = &user_lower {
                        let u_un = u.username.to_ascii_lowercase();
                        if exact {
                            if &u_un != un {
                                return false;
                            }
                        } else if !u_un.contains(un) {
                            return false;
                        }
                    }
                    // Keycloak's `search` leaves service accounts out.
                    if search_lower.is_some() && u.service_account_client_id.is_some() {
                        return false;
                    }
                    if let Some(q) = &search_lower
                        && !q.is_empty()
                    {
                        let hay = format!(
                            "{} {} {} {}",
                            u.username,
                            u.email.as_deref().unwrap_or_default(),
                            u.first_name.as_deref().unwrap_or_default(),
                            u.last_name.as_deref().unwrap_or_default(),
                        )
                        .to_ascii_lowercase();
                        if !hay.contains(q) {
                            return false;
                        }
                    }
                    true
                })
                .cloned()
                .collect();
            out.sort_by_cached_key(|u| u.username.to_ascii_lowercase());
            Ok(out
                .into_iter()
                .skip(first)
                .take(max.clamp(1, MAX_PAGE))
                .collect())
        })
    }

    fn find_users<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        filter: &'a [FieldMatch<PersonField>],
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, ListPage<UserRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let mut found: Vec<&UserRepresentation> = st
                .users
                .iter()
                .filter(|u| Self::user_visible(&st, realm_id, allowed_orgs, u))
                .filter(|u| filter.iter().all(|m| m.matches(u)))
                .collect();
            found.sort_by_cached_key(|u| u.username.to_ascii_lowercase());
            Ok(ListPage {
                total: found.len(),
                items: found
                    .into_iter()
                    .skip(first)
                    .take(max.min(MAX_PAGE))
                    .cloned()
                    .collect(),
            })
        })
    }

    fn upsert_credential<'a>(&'a self, mut cred: CredentialRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if cred.id.is_empty() {
                cred.id = uuid::Uuid::now_v7().to_string();
            }
            if cred.created_date == 0 {
                cred.created_date = TimestampTz::now().as_millis();
            }
            let mut st = self.inner.write();
            st.credentials
                .retain(|c| !(c.realm_id == cred.realm_id && c.id == cred.id));
            // Keep password history up to 5 entries if type == "password-history"
            if cred.credential_type == "password"
                && let Some(prev) = st
                    .credentials
                    .iter()
                    .find(|c| {
                        c.realm_id == cred.realm_id
                            && c.user_id == cred.user_id
                            && c.credential_type == "password"
                    })
                    .cloned()
            {
                st.credentials.retain(|c| {
                    !(c.realm_id == cred.realm_id
                        && c.user_id == cred.user_id
                        && c.credential_type == "password")
                });
                let mut hist = prev;
                hist.id = uuid::Uuid::now_v7().to_string();
                hist.credential_type = "password-history".into();
                st.credentials.insert(0, hist);
            }
            st.credentials.push(cred);
            Ok(())
        })
    }

    fn list_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<CredentialRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .credentials
                .iter()
                .filter(|c| c.realm_id == realm_id && c.user_id == user_id)
                .cloned()
                .collect())
        })
    }

    fn delete_credential<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        cred_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let before = st.credentials.len();
            st.credentials
                .retain(|c| !(c.realm_id == realm_id && c.user_id == user_id && c.id == cred_id));
            Ok(st.credentials.len() < before)
        })
    }

    fn upsert_group<'a>(
        &'a self,
        mut group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        Box::pin(async move {
            if group.id.is_empty() {
                group.id = uuid::Uuid::now_v7().to_string();
            }
            if group.path.is_empty() {
                group.path = format!("/{}", group.name);
            }
            if group.org_id.is_none()
                && let Some(first_seg) = group
                    .path
                    .strip_prefix('/')
                    .and_then(|p| p.split('/').next())
            {
                let st = self.inner.read();
                if st.organizations.iter().any(|o| {
                    o.realm_id == group.realm_id && (o.id == first_seg || o.alias == first_seg)
                }) {
                    group.org_id = Some(first_seg.to_owned());
                }
            }
            let subs = std::mem::take(&mut group.sub_groups);
            {
                let mut st = self.inner.write();
                if let Some(existing) = st
                    .groups
                    .iter()
                    .find(|g| {
                        g.realm_id == group.realm_id && (g.id == group.id || g.path == group.path)
                    })
                    .cloned()
                {
                    group.id = existing.id;
                }
                st.groups
                    .retain(|g| !(g.realm_id == group.realm_id && g.id == group.id));
                st.groups.push(group.clone());
            }
            for mut sub in subs {
                sub.realm_id = group.realm_id.clone();
                sub.parent_id = Some(group.id.clone());
                if sub.path.is_empty() {
                    sub.path = format!("{}/{}", group.path.trim_end_matches('/'), sub.name);
                }
                if sub.org_id.is_none() {
                    sub.org_id = group.org_id.clone();
                }
                let _ = self.upsert_group(sub).await?;
            }
            Ok(group)
        })
    }

    fn update_group<'a>(
        &'a self,
        mut group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let pos = st
                .groups
                .iter()
                .position(|g| g.realm_id == group.realm_id && g.id == group.id)
                .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
            if st
                .groups
                .iter()
                .enumerate()
                .any(|(idx, g)| idx != pos && g.realm_id == group.realm_id && g.path == group.path)
            {
                return Err(KaisatsuError::Conflict(
                    "another group already holds that path".into(),
                ));
            }
            group.realm_roles.clone_from(&st.groups[pos].realm_roles);
            group.client_roles.clone_from(&st.groups[pos].client_roles);
            group.sub_groups.clear();
            st.groups[pos] = group.clone();
            Ok(group)
        })
    }

    fn get_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let Some(g) = st
                .groups
                .iter()
                .find(|g| g.realm_id == realm_id && g.id == group_id)
            else {
                return Ok(None);
            };
            if !Self::org_allowed(allowed_orgs, g.org_id.as_deref()) {
                return Ok(None);
            }
            Ok(Some(g.clone()))
        })
    }

    fn get_group_by_path<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        path: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let norm = if path.starts_with('/') {
                path.to_owned()
            } else {
                format!("/{path}")
            };
            let Some(g) = st
                .groups
                .iter()
                .find(|g| g.realm_id == realm_id && g.path == norm)
            else {
                return Ok(None);
            };
            if !Self::org_allowed(allowed_orgs, g.org_id.as_deref()) {
                return Ok(None);
            }
            Ok(Some(g.clone()))
        })
    }

    fn list_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        exact: bool,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let visible: Vec<GroupRepresentation> = st
                .groups
                .iter()
                .filter(|g| {
                    g.realm_id == realm_id && Self::org_allowed(allowed_orgs, g.org_id.as_deref())
                })
                .cloned()
                .collect();

            // Build nested tree for subgroups so `/groups?search=members&exact=true` returns
            // root groups with `subGroups` populated (matching Portal `people.rs:503` `GroupNode`).
            let mut roots: Vec<GroupRepresentation> = visible
                .iter()
                .filter(|g| {
                    g.parent_id.is_none()
                        && g.path.strip_prefix('/').is_some_and(|p| !p.contains('/'))
                })
                .cloned()
                .collect();
            roots.sort_by(|a, b| a.name.cmp(&b.name));
            for r in &mut roots {
                attach_children(r, &visible);
            }

            let Some(q) = search.filter(|s| !s.is_empty()) else {
                return Ok(roots);
            };
            let q_lower = q.to_ascii_lowercase();
            fn matches_or_has_matching_child(
                node: &mut GroupRepresentation,
                q_lower: &str,
                exact: bool,
            ) -> bool {
                let self_matches = if exact {
                    node.name.eq_ignore_ascii_case(q_lower)
                } else {
                    node.name.to_ascii_lowercase().contains(q_lower)
                };
                node.sub_groups
                    .retain_mut(|ch| matches_or_has_matching_child(ch, q_lower, exact));
                self_matches || !node.sub_groups.is_empty()
            }
            roots.retain_mut(|r| matches_or_has_matching_child(r, &q_lower, exact));
            Ok(roots)
        })
    }

    fn list_child_groups<'a>(
        &'a self,
        allowed_orgs: Option<&'a [String]>,
        parent: &'a GroupRepresentation,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        Box::pin(async move {
            let below = format!("{}/", parent.path.trim_end_matches('/'));
            let st = self.inner.read();
            let subtree: Vec<GroupRepresentation> = st
                .groups
                .iter()
                .filter(|g| {
                    g.realm_id == parent.realm_id
                        && g.path.starts_with(&below)
                        && Self::org_allowed(allowed_orgs, g.org_id.as_deref())
                })
                .cloned()
                .collect();
            let mut top = parent.clone();
            attach_children(&mut top, &subtree);
            Ok(top.sub_groups)
        })
    }

    fn find_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        filter: &'a [FieldMatch<GroupField>],
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, ListPage<GroupWithMembers>> {
        Box::pin(async move {
            let st = self.inner.read();
            let mut found: Vec<&GroupRepresentation> = st
                .groups
                .iter()
                .filter(|g| {
                    g.realm_id == realm_id
                        && Self::org_allowed(allowed_orgs, g.org_id.as_deref())
                        && filter.iter().all(|m| m.matches(g))
                })
                .collect();
            found.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(ListPage {
                total: found.len(),
                items: found
                    .into_iter()
                    .skip(first)
                    .take(max.min(MAX_PAGE))
                    .map(|g| GroupWithMembers {
                        group: g.clone(),
                        members: members_of(&st, g, 0, MAX_PAGE),
                    })
                    .collect(),
            })
        })
    }

    fn delete_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(pos) = st
                .groups
                .iter()
                .position(|g| g.realm_id == realm_id && g.id == group_id)
            else {
                return Ok(false);
            };
            if !Self::org_allowed(allowed_orgs, st.groups[pos].org_id.as_deref()) {
                return Ok(false);
            }
            let mut tree = vec![(st.groups[pos].id.clone(), st.groups[pos].path.clone())];
            let mut next = 0;
            while let Some((id, path)) = tree.get(next).cloned() {
                next += 1;
                let below = format!("{path}/");
                for g in st.groups.iter().filter(|g| g.realm_id == realm_id) {
                    let child = match &g.parent_id {
                        Some(parent) => *parent == id,
                        None => g.path.starts_with(&below),
                    };
                    if !child || tree.iter().any(|(seen, _)| *seen == g.id) {
                        continue;
                    }
                    if !Self::org_allowed(allowed_orgs, g.org_id.as_deref()) {
                        return Err(KaisatsuError::Conflict(
                            "a group below this one is outside your organizations: a realm admin deletes it"
                                .into(),
                        ));
                    }
                    tree.push((g.id.clone(), g.path.clone()));
                }
            }
            let gone = |id: &str| tree.iter().any(|(g, _)| g == id);
            st.groups
                .retain(|g| !(g.realm_id == realm_id && gone(&g.id)));
            st.group_members
                .retain(|gm| !(gm.realm_id == realm_id && gone(&gm.group_id)));
            st.role_mappings.retain(|rm| {
                !(rm.realm_id == realm_id && rm.principal_kind == "group" && gone(&rm.principal_id))
            });
            Ok(true)
        })
    }

    fn add_user_to_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let group = st
                .groups
                .iter()
                .find(|g| g.realm_id == realm_id && g.id == group_id)
                .cloned()
                .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
            if !st
                .users
                .iter()
                .any(|u| u.realm_id == realm_id && u.id == user_id)
            {
                return Err(KaisatsuError::NotFound("user not found".into()));
            }
            if !st.group_members.iter().any(|gm| {
                gm.realm_id == realm_id && gm.user_id == user_id && gm.group_id == group_id
            }) {
                st.group_members.push(GroupMemberRecord {
                    id: uuid::Uuid::now_v7().to_string(),
                    realm_id: realm_id.to_owned(),
                    org_id: group.org_id.clone(),
                    group_id: group_id.to_owned(),
                    user_id: user_id.to_owned(),
                });
            }
            Ok(())
        })
    }

    fn remove_user_from_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.group_members.retain(|gm| {
                !(gm.realm_id == realm_id && gm.user_id == user_id && gm.group_id == group_id)
            });
            Ok(())
        })
    }

    fn list_user_groups<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let member_group_ids: Vec<&str> = st
                .group_members
                .iter()
                .filter(|gm| gm.realm_id == realm_id && gm.user_id == user_id)
                .map(|gm| gm.group_id.as_str())
                .collect();
            let mut groups: Vec<GroupRepresentation> = st
                .groups
                .iter()
                .filter(|g| g.realm_id == realm_id && member_group_ids.contains(&g.id.as_str()))
                .cloned()
                .collect();
            groups.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(groups)
        })
    }

    fn list_group_members<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let Some(group) = st
                .groups
                .iter()
                .find(|g| g.realm_id == realm_id && g.id == group_id)
            else {
                return Err(KaisatsuError::NotFound("group not found".into()));
            };
            if !Self::org_allowed(allowed_orgs, group.org_id.as_deref()) {
                return Err(KaisatsuError::NotFound("group not found".into()));
            }
            Ok(members_of(&st, group, first, max.clamp(1, MAX_PAGE)))
        })
    }

    fn upsert_role<'a>(
        &'a self,
        mut role: RoleRepresentation,
    ) -> StoreFuture<'a, RoleRepresentation> {
        Box::pin(async move {
            if role.id.is_empty() {
                role.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            if let Some(existing) = st.roles.iter().find(|r| {
                r.realm_id == role.realm_id
                    && r.container_id == role.container_id
                    && r.name == role.name
            }) {
                role.id = existing.id.clone();
            }
            st.roles.retain(|r| {
                !(r.realm_id == role.realm_id
                    && r.container_id == role.container_id
                    && r.name == role.name)
            });
            st.roles.push(role.clone());
            Ok(role)
        })
    }

    fn list_roles<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .roles
                .iter()
                .filter(|r| {
                    r.realm_id == realm_id
                        && match container_id {
                            Some(cid) => r.client_role && r.container_id.as_deref() == Some(cid),
                            None => !r.client_role,
                        }
                })
                .cloned()
                .collect())
        })
    }

    fn delete_role<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let before = st.roles.len();
            st.roles.retain(|r| {
                !(r.realm_id == realm_id
                    && r.name == role_name
                    && r.container_id.as_deref() == container_id)
            });
            if st.roles.len() == before {
                return Ok(false);
            }
            match container_id {
                None => {
                    let keep = |names: &mut Vec<String>| names.retain(|n| n != role_name);
                    for u in st.users.iter_mut().filter(|u| u.realm_id == realm_id) {
                        keep(&mut u.realm_roles);
                    }
                    for g in st.groups.iter_mut().filter(|g| g.realm_id == realm_id) {
                        keep(&mut g.realm_roles);
                    }
                    for r in st.roles.iter_mut().filter(|r| r.realm_id == realm_id) {
                        keep(&mut r.composites.realm);
                    }
                    for r in st.realms.values_mut().filter(|r| r.id == realm_id) {
                        keep(&mut r.default_roles);
                    }
                }
                Some(uuid) => {
                    let client_id = st
                        .clients
                        .iter()
                        .find(|c| c.realm_id == realm_id && c.id == uuid)
                        .map(|c| c.client_id.clone());
                    if let Some(client_id) = client_id {
                        for grants in client_grants(&mut st, realm_id) {
                            if let Some(names) = grants.get_mut(&client_id) {
                                names.retain(|n| n != role_name);
                            }
                        }
                    }
                }
            }
            st.role_mappings.retain(|rm| {
                !(rm.realm_id == realm_id
                    && rm.role_name == role_name
                    && rm.client_uuid.as_deref() == container_id)
            });
            Ok(true)
        })
    }

    fn add_role_mapping<'a>(&'a self, mut mapping: RoleMappingRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if mapping.id.is_empty() {
                mapping.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            // A client mapping for a client that does not exist is refused, never stored as a
            // realm role of the same name.
            if let Some(uuid) = &mapping.client_uuid
                && !st
                    .clients
                    .iter()
                    .any(|c| c.realm_id == mapping.realm_id && &c.id == uuid)
            {
                return Err(KaisatsuError::NotFound("client not found".into()));
            }
            let (realm, id) = (&mapping.realm_id, &mapping.principal_id);
            // The person a user mapping is reflected on, found once under this write lock.
            let holder = if mapping.principal_kind == "user" {
                let at = st
                    .users
                    .iter()
                    .position(|u| &u.realm_id == realm && &u.id == id)
                    .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
                Some(at)
            } else if st
                .groups
                .iter()
                .any(|g| &g.realm_id == realm && &g.id == id)
            {
                None
            } else {
                return Err(KaisatsuError::NotFound("group not found".into()));
            };
            st.role_mappings.retain(|rm| {
                !(rm.realm_id == mapping.realm_id
                    && rm.principal_kind == mapping.principal_kind
                    && rm.principal_id == mapping.principal_id
                    && rm.client_uuid == mapping.client_uuid
                    && rm.role_name == mapping.role_name)
            });
            // Also reflect on the user representation for immediate claim evaluation
            let client_id_opt = mapping.client_uuid.as_ref().and_then(|uuid| {
                st.clients
                    .iter()
                    .find(|c| c.realm_id == mapping.realm_id && &c.id == uuid)
                    .map(|c| c.client_id.clone())
            });
            if let Some(u) = holder.and_then(|at| st.users.get_mut(at)) {
                if let Some(cid) = client_id_opt {
                    let list = u.client_roles.entry(cid).or_default();
                    if !list.contains(&mapping.role_name) {
                        list.push(mapping.role_name.clone());
                    }
                } else if !u.realm_roles.contains(&mapping.role_name) {
                    u.realm_roles.push(mapping.role_name.clone());
                }
            }
            st.role_mappings.push(mapping);
            Ok(())
        })
    }

    fn remove_role_mapping<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            if let Some(uuid) = client_uuid
                && !st
                    .clients
                    .iter()
                    .any(|c| c.realm_id == realm_id && c.id == uuid)
            {
                return Err(KaisatsuError::NotFound("client not found".into()));
            }
            st.role_mappings.retain(|rm| {
                !(rm.realm_id == realm_id
                    && rm.principal_kind == principal_kind
                    && rm.principal_id == principal_id
                    && rm.client_uuid.as_deref() == client_uuid
                    && rm.role_name == role_name)
            });
            if principal_kind == "user" {
                let client_id_opt = client_uuid.and_then(|uuid| {
                    st.clients
                        .iter()
                        .find(|c| c.realm_id == realm_id && c.id == uuid)
                        .map(|c| c.client_id.clone())
                });
                if let Some(u) = st
                    .users
                    .iter_mut()
                    .find(|u| u.realm_id == realm_id && u.id == principal_id)
                {
                    if let Some(cid) = client_id_opt {
                        if let Some(list) = u.client_roles.get_mut(&cid) {
                            list.retain(|r| r != role_name);
                        }
                    } else {
                        u.realm_roles.retain(|r| r != role_name);
                    }
                }
            }
            Ok(())
        })
    }

    fn list_role_mappings_for_principal<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleMappingRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .role_mappings
                .iter()
                .filter(|rm| {
                    rm.realm_id == realm_id
                        && rm.principal_kind == principal_kind
                        && rm.principal_id == principal_id
                        && rm.client_uuid.as_deref() == client_uuid
                })
                .cloned()
                .collect())
        })
    }

    fn list_principals_with_client_role<'a>(
        &'a self,
        realm_id: &'a str,
        client_uuid: &'a str,
        role_name: &'a str,
        principal_kind: &'a str,
    ) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .role_mappings
                .iter()
                .filter(|rm| {
                    rm.realm_id == realm_id
                        && rm.client_uuid.as_deref() == Some(client_uuid)
                        && rm.role_name == role_name
                        && rm.principal_kind == principal_kind
                })
                .map(|rm| rm.principal_id.clone())
                .collect())
        })
    }

    fn upsert_client<'a>(
        &'a self,
        mut client: ClientRepresentation,
    ) -> StoreFuture<'a, ClientRepresentation> {
        Box::pin(async move {
            if client.id.is_empty() {
                client.id = uuid::Uuid::now_v7().to_string();
            }
            for m in &mut client.protocol_mappers {
                if m.id.is_empty() {
                    m.id = uuid::Uuid::now_v7().to_string();
                }
                m.realm_id = client.realm_id.clone();
            }
            let mut st = self.inner.write();
            if let Some(existing) = st
                .clients
                .iter()
                .find(|c| c.realm_id == client.realm_id && c.id == client.id)
                .or_else(|| {
                    st.clients
                        .iter()
                        .find(|c| c.realm_id == client.realm_id && c.client_id == client.client_id)
                })
                .cloned()
            {
                if existing.client_id != client.client_id {
                    let (realm, new) = (client.realm_id.as_str(), client.client_id.as_str());
                    let account = format!("service-account-{new}").to_ascii_lowercase();
                    if st
                        .clients
                        .iter()
                        .any(|c| c.realm_id == realm && c.client_id == new)
                        || st
                            .users
                            .iter()
                            .any(|u| u.realm_id == realm && u.username == account)
                    {
                        return Err(KaisatsuError::Conflict(
                            "another client or person already holds that client id".into(),
                        ));
                    }
                    let old = existing.client_id.as_str();
                    for grants in client_grants(&mut st, realm) {
                        if let Some(names) = grants.remove(old) {
                            grants.insert(new.to_owned(), names);
                        }
                    }
                    for u in st.users.iter_mut().filter(|u| {
                        u.realm_id == realm && u.service_account_client_id.as_deref() == Some(old)
                    }) {
                        u.username.clone_from(&account);
                        u.service_account_client_id = Some(new.to_owned());
                    }
                    st.consents
                        .retain(|c| !(c.realm_id == realm && c.client_id == old));
                }
                client.id = existing.id;
                if client.secret.is_none() {
                    client.secret = existing.secret;
                }
            } else if !client.public_client && client.secret.is_none() {
                client.secret = Some(kaisatsu_crypto::random_handle_256());
            }
            if let Some(mark) = client.attributes.get("managed-by") {
                client.managed_by = Some(mark.clone());
            }
            st.clients
                .retain(|c| !(c.realm_id == client.realm_id && c.id == client.id));
            st.clients.push(client.clone());

            // W3: confidential client with serviceAccountsEnabled gets `service-account-{clientId}`
            if !client.public_client && client.service_accounts_enabled {
                let sa_username = format!("service-account-{}", client.client_id);
                let exists = st.users.iter().any(|u| {
                    u.realm_id == client.realm_id && u.username.eq_ignore_ascii_case(&sa_username)
                });
                if !exists {
                    st.users.push(UserRepresentation {
                        id: uuid::Uuid::now_v7().to_string(),
                        realm_id: client.realm_id.clone(),
                        org_id: client.org_id.clone(),
                        username: sa_username,
                        enabled: true,
                        email_verified: false,
                        service_account_client_id: Some(client.client_id.clone()),
                        created_timestamp: Some(TimestampTz::now().as_millis()),
                        ..Default::default()
                    });
                }
            }
            Ok(client)
        })
    }

    fn get_client_by_uuid<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let Some(c) = st
                .clients
                .iter()
                .find(|c| c.realm_id == realm_id && c.id == uuid)
            else {
                return Ok(None);
            };
            if !Self::org_allowed(allowed_orgs, c.org_id.as_deref()) {
                return Ok(None);
            }
            Ok(Some(c.clone()))
        })
    }

    fn get_client_by_client_id<'a>(
        &'a self,
        realm_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .clients
                .iter()
                .find(|c| c.realm_id == realm_id && c.client_id == client_id)
                .cloned())
        })
    }

    fn list_clients<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        client_id_filter: Option<&'a str>,
        search: bool,
    ) -> StoreFuture<'a, Vec<ClientRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            let out: Vec<ClientRepresentation> = st
                .clients
                .iter()
                .filter(|c| {
                    c.realm_id == realm_id && Self::org_allowed(allowed_orgs, c.org_id.as_deref())
                })
                .filter(|c| match client_id_filter {
                    None => true,
                    Some(f) if search => c.client_id.contains(f),
                    Some(f) => c.client_id == f,
                })
                .cloned()
                .collect();
            Ok(out)
        })
    }

    fn delete_client<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(pos) = st
                .clients
                .iter()
                .position(|c| c.realm_id == realm_id && c.id == uuid)
            else {
                return Ok(false);
            };
            if !Self::org_allowed(allowed_orgs, st.clients[pos].org_id.as_deref()) {
                return Ok(false);
            }
            let removed = st.clients.remove(pos);
            for grants in client_grants(&mut st, realm_id) {
                grants.remove(&removed.client_id);
            }
            st.roles.retain(|r| {
                !(r.realm_id == realm_id
                    && r.client_role
                    && r.container_id.as_deref() == Some(uuid))
            });
            st.consents
                .retain(|c| !(c.realm_id == realm_id && c.client_id == removed.client_id));
            st.role_mappings
                .retain(|rm| !(rm.realm_id == realm_id && rm.client_uuid.as_deref() == Some(uuid)));
            let sa_user = format!("service-account-{}", removed.client_id);
            // Only the client's own service account goes with it, never a person of that name.
            if let Some(pos) = st.users.iter().position(|u| {
                u.realm_id == realm_id
                    && u.username.eq_ignore_ascii_case(&sa_user)
                    && u.service_account_client_id.as_deref() == Some(removed.client_id.as_str())
            }) {
                let account = st.users.remove(pos);
                drop_person_rows(&mut st, realm_id, &account.id);
            }
            Ok(true)
        })
    }

    fn upsert_client_scope<'a>(
        &'a self,
        mut scope: ClientScopeRepresentation,
    ) -> StoreFuture<'a, ClientScopeRepresentation> {
        Box::pin(async move {
            if scope.id.is_empty() {
                scope.id = uuid::Uuid::now_v7().to_string();
            }
            for m in &mut scope.protocol_mappers {
                if m.id.is_empty() {
                    m.id = uuid::Uuid::now_v7().to_string();
                }
                m.realm_id = scope.realm_id.clone();
            }
            if let Some(mark) = scope.attributes.get("managed-by") {
                scope.managed_by = Some(mark.clone());
            }
            let mut st = self.inner.write();
            if let Some(existing) = st.client_scopes.iter().find(|s| {
                s.realm_id == scope.realm_id && (s.id == scope.id || s.name == scope.name)
            }) {
                scope.id = existing.id.clone();
            }
            st.client_scopes
                .retain(|s| !(s.realm_id == scope.realm_id && s.id == scope.id));
            st.client_scopes.push(scope.clone());
            Ok(scope)
        })
    }

    fn get_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientScopeRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .client_scopes
                .iter()
                .find(|s| s.realm_id == realm_id && (s.id == scope_id || s.name == scope_id))
                .cloned())
        })
    }

    fn list_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .client_scopes
                .iter()
                .filter(|s| s.realm_id == realm_id)
                .cloned()
                .collect())
        })
    }

    fn get_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .client_scopes
                .iter()
                .filter(|s| {
                    s.realm_id == realm_id && (keys.contains(&s.name) || keys.contains(&s.id))
                })
                .cloned()
                .collect())
        })
    }

    fn list_client_scopes_with_prefix<'a>(
        &'a self,
        realm_id: &'a str,
        prefix: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .client_scopes
                .iter()
                .filter(|s| s.realm_id == realm_id && s.name.starts_with(prefix))
                .cloned()
                .collect())
        })
    }

    fn delete_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(pos) = st
                .client_scopes
                .iter()
                .position(|s| s.realm_id == realm_id && s.id == scope_id)
            else {
                return Ok(false);
            };
            let removed = st.client_scopes.remove(pos);
            for c in st.clients.iter_mut().filter(|c| c.realm_id == realm_id) {
                c.default_client_scopes.retain(|n| n != &removed.name);
                c.optional_client_scopes.retain(|n| n != &removed.name);
            }
            Ok(true)
        })
    }

    fn upsert_idp<'a>(&'a self, mut idp: IdentityProviderRepresentation) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if idp.internal_id.is_empty() {
                idp.internal_id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.idps
                .retain(|i| !(i.realm_id == idp.realm_id && i.alias == idp.alias));
            st.idps.push(idp);
            Ok(())
        })
    }

    fn get_idp<'a>(
        &'a self,
        realm_id: &'a str,
        alias: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .idps
                .iter()
                .find(|i| i.realm_id == realm_id && i.alias == alias)
                .cloned())
        })
    }

    fn list_idps<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderRepresentation>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .idps
                .iter()
                .filter(|i| i.realm_id == realm_id)
                .cloned()
                .collect())
        })
    }

    fn upsert_signing_key<'a>(&'a self, key: SigningKeyRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.signing_keys
                .retain(|k| !(k.realm_id == key.realm_id && k.kid == key.kid));
            st.signing_keys.push(key);
            Ok(())
        })
    }

    fn list_signing_keys<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<SigningKeyRecord>> {
        Box::pin(async move {
            let _ = self.ensure_realm_keys(realm_id);
            let st = self.inner.read();
            let mut keys: Vec<SigningKeyRecord> = st
                .signing_keys
                .iter()
                .filter(|k| k.realm_id == realm_id)
                .cloned()
                .collect();
            keys.sort_by_key(|k| std::cmp::Reverse(k.priority));
            Ok(keys)
        })
    }

    fn ensure_signing_key<'a>(
        &'a self,
        realm_id: &'a str,
        algorithm: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move { self.ensure_key(realm_id, algorithm) })
    }

    fn put_auth_session<'a>(&'a self, session: AuthSession) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.auth_sessions
                .retain(|s| !(s.realm_id == session.realm_id && s.id == session.id));
            st.auth_sessions.push(session);
            Ok(())
        })
    }

    fn get_auth_session_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
    ) -> StoreFuture<'a, Option<AuthSession>> {
        Box::pin(async move {
            let st = self.inner.read();
            let now = TimestampTz::now();
            Ok(st
                .auth_sessions
                .iter()
                .find(|s| s.realm_id == realm_id && s.id == id && s.expires_at > now)
                .cloned())
        })
    }

    fn delete_auth_session<'a>(&'a self, realm_id: &'a str, id: &'a str) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.auth_sessions
                .retain(|s| !(s.realm_id == realm_id && s.id == id));
            Ok(())
        })
    }

    fn put_auth_code<'a>(&'a self, code: AuthorizationCodeRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.auth_codes.push(code);
            Ok(())
        })
    }

    fn consume_auth_code<'a>(
        &'a self,
        realm_id: &'a str,
        code_hash: &'a str,
    ) -> StoreFuture<'a, (Option<AuthorizationCodeRecord>, bool)> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(rec) = st
                .auth_codes
                .iter_mut()
                .find(|c| c.realm_id == realm_id && c.code_hash == code_hash)
            else {
                return Ok((None, false));
            };
            if rec.consumed {
                return Ok((Some(rec.clone()), true));
            }
            rec.consumed = true;
            Ok((Some(rec.clone()), false))
        })
    }

    fn put_device_code<'a>(&'a self, rec: DeviceCodeRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            if st.device_codes.iter().any(|d| {
                d.realm_id == rec.realm_id
                    && (d.user_code_hash == rec.user_code_hash
                        || d.device_code_hash == rec.device_code_hash)
            }) {
                return Err(KaisatsuError::Conflict("device code exists".into()));
            }
            st.device_codes.push(rec);
            Ok(())
        })
    }

    fn get_device_code_by_user_code<'a>(
        &'a self,
        realm_id: &'a str,
        user_code_hash: &'a str,
    ) -> StoreFuture<'a, Option<DeviceCodeRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .device_codes
                .iter()
                .find(|d| d.realm_id == realm_id && d.user_code_hash == user_code_hash)
                .cloned())
        })
    }

    fn decide_device_code<'a>(&'a self, rec: DeviceCodeRecord) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(stored) = st
                .device_codes
                .iter_mut()
                .find(|d| d.realm_id == rec.realm_id && d.device_code_hash == rec.device_code_hash)
            else {
                return Ok(false);
            };
            if stored.status != DeviceStatus::Pending || stored.consumed {
                return Ok(false);
            }
            *stored = rec;
            Ok(true)
        })
    }

    fn poll_device_code<'a>(
        &'a self,
        realm_id: &'a str,
        device_code_hash: &'a str,
        client_id: &'a str,
        now: jc_time::TimestampTz,
    ) -> StoreFuture<'a, Option<(DeviceCodeRecord, DevicePoll)>> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(stored) = st.device_codes.iter_mut().find(|d| {
                d.realm_id == realm_id
                    && d.device_code_hash == device_code_hash
                    && d.client_id == client_id
            }) else {
                return Ok(None);
            };
            let before = stored.clone();
            let (next, answer) = before.polled(now);
            *stored = next;
            Ok(Some((before, answer)))
        })
    }

    fn put_par_request<'a>(&'a self, par: ParRequestRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.par_requests.push(par);
            Ok(())
        })
    }

    fn consume_par_request<'a>(
        &'a self,
        realm_id: &'a str,
        request_uri_hash: &'a str,
    ) -> StoreFuture<'a, Option<ParRequestRecord>> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let now = TimestampTz::now();
            let Some(rec) = st.par_requests.iter_mut().find(|p| {
                p.realm_id == realm_id
                    && p.request_uri_hash == request_uri_hash
                    && !p.consumed
                    && p.expires_at > now
            }) else {
                return Ok(None);
            };
            rec.consumed = true;
            Ok(Some(rec.clone()))
        })
    }

    fn upsert_sso_session<'a>(&'a self, session: SsoSession) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.sso_sessions
                .retain(|s| !(s.realm_id == session.realm_id && s.id == session.id));
            for cid in session.clients.keys() {
                if !st.client_sessions.iter().any(|cs| {
                    cs.realm_id == session.realm_id
                        && cs.sso_session_id == session.id
                        && &cs.client_id == cid
                }) {
                    st.client_sessions.push(ClientSession {
                        id: uuid::Uuid::now_v7().to_string(),
                        realm_id: session.realm_id.clone(),
                        org_id: session.org_id.clone(),
                        sso_session_id: session.id.clone(),
                        client_id: cid.clone(),
                        user_id: session.user_id.clone(),
                        scopes: vec!["openid".into()],
                        created_at: TimestampTz::now(),
                        last_access: TimestampTz::now(),
                    });
                }
            }
            st.sso_sessions.push(session);
            Ok(())
        })
    }

    fn get_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        Box::pin(async move {
            let st = self.inner.read();
            let now = TimestampTz::now();
            Ok(st
                .sso_sessions
                .iter()
                .find(|s| {
                    s.realm_id == realm_id
                        && s.id == session_id
                        && s.idle_expires_at > now
                        && s.max_expires_at > now
                })
                .cloned())
        })
    }

    fn get_sso_session_by_handle<'a>(
        &'a self,
        realm_id: &'a str,
        handle_hash: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        Box::pin(async move {
            let st = self.inner.read();
            let now = TimestampTz::now();
            Ok(st
                .sso_sessions
                .iter()
                .find(|s| {
                    s.realm_id == realm_id
                        && s.handle_hash == handle_hash
                        && s.idle_expires_at > now
                        && s.max_expires_at > now
                })
                .cloned())
        })
    }

    fn list_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        Box::pin(async move {
            let st = self.inner.read();
            let now = TimestampTz::now();
            Ok(st
                .sso_sessions
                .iter()
                .filter(|s| {
                    s.realm_id == realm_id
                        && s.user_id == user_id
                        && s.idle_expires_at > now
                        && s.max_expires_at > now
                })
                .cloned()
                .collect())
        })
    }

    fn delete_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(pos) = st
                .sso_sessions
                .iter()
                .position(|s| s.realm_id == realm_id && s.id == session_id)
            else {
                return Ok(None);
            };
            let removed = st.sso_sessions.remove(pos);
            st.client_sessions
                .retain(|cs| !(cs.realm_id == realm_id && cs.sso_session_id == session_id));
            // An offline family outlives the SSO session it began in (S3-22).
            for rf in st.refresh_families.iter_mut().filter(|rf| {
                rf.realm_id == realm_id && rf.sso_session_id == session_id && !rf.offline
            }) {
                rf.revoked = true;
            }
            Ok(Some(removed))
        })
    }

    fn revoke_offline_families_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            for rf in st
                .refresh_families
                .iter_mut()
                .filter(|rf| rf.realm_id == realm_id && rf.user_id == user_id && rf.offline)
            {
                rf.revoked = true;
            }
            Ok(())
        })
    }

    fn delete_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let (removed, kept): (Vec<_>, Vec<_>) = st
                .sso_sessions
                .drain(..)
                .partition(|s| s.realm_id == realm_id && s.user_id == user_id);
            st.sso_sessions = kept;
            st.client_sessions
                .retain(|cs| !(cs.realm_id == realm_id && cs.user_id == user_id));
            for rf in st
                .refresh_families
                .iter_mut()
                .filter(|rf| rf.realm_id == realm_id && rf.user_id == user_id)
            {
                rf.revoked = true;
            }
            Ok(removed)
        })
    }

    fn upsert_offline_session<'a>(&'a self, session: OfflineSession) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.offline_sessions
                .retain(|s| !(s.realm_id == session.realm_id && s.id == session.id));
            st.offline_sessions.push(session);
            Ok(())
        })
    }

    fn delete_user_client_offline_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.offline_sessions.retain(|s| {
                !(s.realm_id == realm_id && s.user_id == user_id && s.client_id == client_id)
            });
            for rf in st.refresh_families.iter_mut().filter(|rf| {
                rf.realm_id == realm_id && rf.user_id == user_id && rf.client_id == client_id
            }) {
                rf.revoked = true;
            }
            Ok(())
        })
    }

    fn upsert_refresh_family<'a>(&'a self, family: RefreshTokenFamily) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.refresh_families
                .retain(|f| !(f.realm_id == family.realm_id && f.id == family.id));
            st.refresh_families.push(family);
            Ok(())
        })
    }

    fn get_refresh_family<'a>(
        &'a self,
        realm_id: &'a str,
        family_id: &'a str,
    ) -> StoreFuture<'a, Option<RefreshTokenFamily>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .refresh_families
                .iter()
                .find(|f| f.realm_id == realm_id && f.id == family_id)
                .cloned())
        })
    }

    fn rotate_refresh_family<'a>(
        &'a self,
        family: RefreshTokenFamily,
        expected_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(current) = st
                .refresh_families
                .iter_mut()
                .find(|f| f.realm_id == family.realm_id && f.id == family.id)
            else {
                return Ok(false);
            };
            if current.revoked || current.current_jti_hash != expected_hash {
                return Ok(false);
            }
            *current = family;
            Ok(true)
        })
    }

    fn revoke_refresh_families_for_session<'a>(
        &'a self,
        realm_id: &'a str,
        sso_session_id: &'a str,
        client_id: Option<&'a str>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            for rf in st.refresh_families.iter_mut().filter(|rf| {
                rf.realm_id == realm_id
                    && rf.sso_session_id == sso_session_id
                    && client_id.is_none_or(|cid| rf.client_id == cid)
            }) {
                rf.revoked = true;
            }
            if let Some(cid) = client_id {
                st.client_sessions.retain(|cs| {
                    !(cs.realm_id == realm_id
                        && cs.sso_session_id == sso_session_id
                        && cs.client_id == cid)
                });
            }
            Ok(())
        })
    }

    fn mark_token_revoked<'a>(&'a self, rec: RevokedTokenRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            if st
                .revoked_tokens
                .iter()
                .any(|r| r.realm_id == rec.realm_id && r.jti_hash == rec.jti_hash)
            {
                return Err(KaisatsuError::Conflict(
                    "token or jti already used/revoked".into(),
                ));
            }
            st.revoked_tokens.push(rec);
            Ok(())
        })
    }

    fn is_token_revoked<'a>(
        &'a self,
        realm_id: &'a str,
        jti_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let st = self.inner.read();
            let now = TimestampTz::now();
            Ok(st
                .revoked_tokens
                .iter()
                .any(|r| r.realm_id == realm_id && r.jti_hash == jti_hash && r.expires_at > now))
        })
    }

    fn get_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, Option<LoginFailureRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .login_failures
                .iter()
                .find(|f| f.realm_id == realm_id && f.target_key == target_key)
                .cloned())
        })
    }

    fn upsert_login_failure<'a>(&'a self, rec: LoginFailureRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.login_failures
                .retain(|f| !(f.realm_id == rec.realm_id && f.target_key == rec.target_key));
            st.login_failures.push(rec);
            Ok(())
        })
    }

    fn update_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
        update: &'a LoginFailureUpdate<'a>,
    ) -> StoreFuture<'a, LoginFailureRecord> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let pos = st
                .login_failures
                .iter()
                .position(|f| f.realm_id == realm_id && f.target_key == target_key);
            let rec = update(pos.map(|i| st.login_failures.swap_remove(i)));
            st.login_failures.push(rec.clone());
            Ok(rec)
        })
    }

    fn clear_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            st.login_failures
                .retain(|f| !(f.realm_id == realm_id && f.target_key == target_key));
            Ok(())
        })
    }

    fn upsert_consent<'a>(&'a self, mut consent: ConsentRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if consent.id.is_empty() {
                consent.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.consents.retain(|c| {
                !(c.realm_id == consent.realm_id
                    && c.user_id == consent.user_id
                    && c.client_id == consent.client_id)
            });
            st.consents.push(consent);
            Ok(())
        })
    }

    fn list_consents<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<ConsentRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .consents
                .iter()
                .filter(|c| c.realm_id == realm_id && c.user_id == user_id)
                .cloned()
                .collect())
        })
    }

    fn delete_consent<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let before = st.consents.len();
            st.consents.retain(|c| {
                !(c.realm_id == realm_id && c.user_id == user_id && c.client_id == client_id)
            });
            st.offline_sessions.retain(|s| {
                !(s.realm_id == realm_id && s.user_id == user_id && s.client_id == client_id)
            });
            for rf in st.refresh_families.iter_mut().filter(|rf| {
                rf.realm_id == realm_id && rf.user_id == user_id && rf.client_id == client_id
            }) {
                rf.revoked = true;
            }
            Ok(st.consents.len() < before)
        })
    }

    fn upsert_required_action<'a>(&'a self, mut ra: RequiredActionConfig) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if ra.id.is_empty() {
                ra.id = format!("{}:{}", ra.realm_id, ra.alias);
            }
            let mut st = self.inner.write();
            st.required_actions
                .retain(|r| !(r.realm_id == ra.realm_id && r.alias == ra.alias));
            st.required_actions.push(ra);
            Ok(())
        })
    }

    fn list_required_actions<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<RequiredActionConfig>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .required_actions
                .iter()
                .filter(|r| r.realm_id == realm_id)
                .cloned()
                .collect())
        })
    }

    fn upsert_branding<'a>(&'a self, branding: BrandingBlock) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let branding = branding.sanitised();
            let mut st = self.inner.write();
            st.brandings
                .retain(|b| !(b.realm_id == branding.realm_id && b.org_id == branding.org_id));
            st.brandings.push(branding);
            Ok(())
        })
    }

    fn get_branding<'a>(
        &'a self,
        realm_id: &'a str,
        org_id: Option<&'a str>,
    ) -> StoreFuture<'a, Option<BrandingBlock>> {
        Box::pin(async move {
            let st = self.inner.read();
            if let Some(org) = org_id
                && let Some(b) = st
                    .brandings
                    .iter()
                    .find(|b| b.realm_id == realm_id && b.org_id.as_deref() == Some(org))
            {
                return Ok(Some(b.clone()));
            }
            Ok(st
                .brandings
                .iter()
                .find(|b| b.realm_id == realm_id && b.org_id.is_none())
                .cloned())
        })
    }

    fn append_event<'a>(&'a self, mut event: AuditEvent) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if event.id.is_empty() {
                event.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.events.push(event);
            Ok(())
        })
    }

    fn list_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AuditEvent>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .events
                .iter()
                .filter(|e| {
                    e.realm_id == realm_id && Self::org_allowed(allowed_orgs, e.org_id.as_deref())
                })
                .cloned()
                .collect())
        })
    }

    fn append_admin_event<'a>(&'a self, mut event: AdminEvent) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if event.id.is_empty() {
                event.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.admin_events.push(event);
            Ok(())
        })
    }

    fn list_admin_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AdminEvent>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .admin_events
                .iter()
                .filter(|ae| {
                    ae.realm_id == realm_id && Self::org_allowed(allowed_orgs, ae.org_id.as_deref())
                })
                .cloned()
                .collect())
        })
    }

    fn insert_webauthn_credential<'a>(
        &'a self,
        mut cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, WebAuthnCredentialRecord> {
        Box::pin(async move {
            if cred.id.is_empty() {
                cred.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            if st
                .webauthn_credentials
                .iter()
                .any(|w| w.realm_id == cred.realm_id && w.credential_id == cred.credential_id)
            {
                return Err(KaisatsuError::Conflict(
                    "WebAuthn credential_id is already registered in this realm".into(),
                ));
            }
            st.webauthn_credentials.push(cred.clone());
            Ok(cred)
        })
    }

    fn update_webauthn_credential<'a>(
        &'a self,
        cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let item = st
                .webauthn_credentials
                .iter_mut()
                .find(|w| w.realm_id == cred.realm_id && w.id == cred.id)
                .ok_or_else(|| KaisatsuError::NotFound("WebAuthn credential not found".into()))?;
            *item = cred;
            Ok(())
        })
    }

    fn get_webauthn_by_credential_id<'a>(
        &'a self,
        realm_id: &'a str,
        credential_id: &'a str,
    ) -> StoreFuture<'a, Option<WebAuthnCredentialRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .webauthn_credentials
                .iter()
                .find(|w| w.realm_id == realm_id && w.credential_id == credential_id)
                .cloned())
        })
    }

    fn list_webauthn_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<WebAuthnCredentialRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .webauthn_credentials
                .iter()
                .filter(|w| w.realm_id == realm_id && w.user_id == user_id)
                .cloned()
                .collect())
        })
    }

    fn delete_webauthn_credential<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(pos) = st
                .webauthn_credentials
                .iter()
                .position(|w| w.realm_id == realm_id && w.id == id)
            else {
                return Ok(false);
            };
            if !Self::org_allowed(allowed_orgs, st.webauthn_credentials[pos].org_id.as_deref()) {
                return Ok(false);
            }
            st.webauthn_credentials.remove(pos);
            Ok(true)
        })
    }

    fn update_user_with_outbox<'a>(
        &'a self,
        mut user: UserRepresentation,
        mut outbox: EmailOutboxRecord,
    ) -> StoreFuture<'a, UserRepresentation> {
        Box::pin(async move {
            user.username = user.username.to_ascii_lowercase();
            if let Some(em) = &mut user.email {
                *em = em.to_ascii_lowercase();
            }
            if outbox.id.is_empty() {
                outbox.id = uuid::Uuid::now_v7().to_string();
            }
            user.groups.clear();
            let mut st = self.inner.write();
            let pos = st
                .users
                .iter()
                .position(|u| u.realm_id == user.realm_id && u.id == user.id)
                .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
            let dup = st.users.iter().enumerate().any(|(idx, u)| {
                idx != pos
                    && u.realm_id == user.realm_id
                    && (u.username.eq_ignore_ascii_case(&user.username)
                        || (user.email.is_some()
                            && u.email
                                .as_deref()
                                .is_some_and(|e| Some(e) == user.email.as_deref())))
            });
            if dup {
                return Err(KaisatsuError::Conflict(
                    "another user already holds that username or email".into(),
                ));
            }
            user.realm_roles.clone_from(&st.users[pos].realm_roles);
            user.client_roles.clone_from(&st.users[pos].client_roles);
            st.users[pos] = user.clone();
            st.email_outbox.push(outbox);
            Ok(user)
        })
    }

    fn enqueue_outbox<'a>(&'a self, mut outbox: EmailOutboxRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if outbox.id.is_empty() {
                outbox.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.email_outbox.push(outbox);
            Ok(())
        })
    }

    fn claim_outbox_batch<'a>(
        &'a self,
        realm_id: &'a str,
        limit: usize,
        lease: std::time::Duration,
        max_attempts: u32,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        Box::pin(async move {
            let cap = limit.clamp(1, 50);
            let now = TimestampTz::now();
            let mut st = self.inner.write();
            let mut claimed = Vec::new();
            for row in st.email_outbox.iter_mut() {
                if claimed.len() >= cap {
                    break;
                }
                if row.realm_id != realm_id || row.next_attempt_at > now {
                    continue;
                }
                match row.status.as_str() {
                    "sending" if row.attempts >= max_attempts => {
                        row.status = "failed".into();
                        row.last_error = Some(OUTBOX_LEASE_LAPSED.into());
                    }
                    "pending" | "sending" => {
                        row.status = "sending".into();
                        row.attempts = row.attempts.saturating_add(1);
                        row.next_attempt_at =
                            now + SignedDuration::try_from(lease).unwrap_or(SignedDuration::MAX);
                        claimed.push(row.clone());
                    }
                    _ => {}
                }
            }
            Ok(claimed)
        })
    }

    fn complete_outbox_row<'a>(
        &'a self,
        claimed: &'a EmailOutboxRecord,
        sent: bool,
        error: Option<String>,
        next_attempt_at: Option<jc_time::TimestampTz>,
        max_attempts: u32,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(row) = st.email_outbox.iter_mut().find(|r| {
                r.realm_id == claimed.realm_id
                    && r.id == claimed.id
                    && r.status == "sending"
                    && r.attempts == claimed.attempts
            }) else {
                return Ok(false);
            };
            if sent {
                row.status = "sent".into();
                row.sent_at = Some(TimestampTz::now());
                row.last_error = None;
            } else {
                row.last_error = error;
                if row.attempts >= max_attempts {
                    row.status = "failed".into();
                } else {
                    row.status = "pending".into();
                    row.next_attempt_at = next_attempt_at
                        .unwrap_or_else(|| TimestampTz::now() + SignedDuration::from_secs(5));
                }
            }
            Ok(true)
        })
    }

    fn retry_outbox_row<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, Option<EmailOutboxRecord>> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let Some(row) = st
                .email_outbox
                .iter_mut()
                .find(|r| r.realm_id == realm_id && r.id == id)
            else {
                return Ok(None);
            };
            if !Self::org_allowed(allowed_orgs, row.org_id.as_deref()) {
                return Ok(None);
            }
            row.status = "pending".into();
            row.attempts = 0;
            row.last_error = None;
            row.next_attempt_at = TimestampTz::now();
            Ok(Some(row.clone()))
        })
    }

    fn list_outbox_rows<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .email_outbox
                .iter()
                .filter(|r| {
                    r.realm_id == realm_id && Self::org_allowed(allowed_orgs, r.org_id.as_deref())
                })
                .cloned()
                .collect())
        })
    }

    fn upsert_idp_link<'a>(&'a self, mut link: IdentityProviderLinkRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if link.id.is_empty() {
                link.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.idp_links.retain(|l| {
                !(l.realm_id == link.realm_id
                    && l.idp_alias == link.idp_alias
                    && l.external_user_id == link.external_user_id)
            });
            st.idp_links.push(link);
            Ok(())
        })
    }

    fn get_idp_link_by_external<'a>(
        &'a self,
        realm_id: &'a str,
        idp_alias: &'a str,
        external_user_id: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderLinkRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .idp_links
                .iter()
                .find(|l| {
                    l.realm_id == realm_id
                        && l.idp_alias == idp_alias
                        && l.external_user_id == external_user_id
                })
                .cloned())
        })
    }

    fn list_idp_links_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderLinkRecord>> {
        Box::pin(async move {
            let st = self.inner.read();
            Ok(st
                .idp_links
                .iter()
                .filter(|l| l.realm_id == realm_id && l.user_id == user_id)
                .cloned()
                .collect())
        })
    }

    fn sweep_expired<'a>(&'a self) -> StoreFuture<'a, u64> {
        Box::pin(async move {
            let mut st = self.inner.write();
            let now = TimestampTz::now();
            let mut removed = 0u64;

            let b1 = st.auth_sessions.len();
            st.auth_sessions.retain(|s| s.expires_at > now);
            removed += (b1 - st.auth_sessions.len()) as u64;

            let b2 = st.auth_codes.len();
            st.auth_codes.retain(|c| c.expires_at > now);
            removed += (b2 - st.auth_codes.len()) as u64;

            let b3 = st.par_requests.len();
            st.par_requests.retain(|p| p.expires_at > now);
            removed += (b3 - st.par_requests.len()) as u64;

            let b4 = st.sso_sessions.len();
            st.sso_sessions
                .retain(|s| s.idle_expires_at > now && s.max_expires_at > now);
            removed += (b4 - st.sso_sessions.len()) as u64;

            let b5 = st.offline_sessions.len();
            st.offline_sessions
                .retain(|s| s.idle_expires_at > now && s.max_expires_at > now);
            removed += (b5 - st.offline_sessions.len()) as u64;

            let b6 = st.revoked_tokens.len();
            st.revoked_tokens.retain(|r| r.expires_at > now);
            removed += (b6 - st.revoked_tokens.len()) as u64;

            let _ = SignedDuration::from_secs(0);
            Ok(removed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: &str = "r";

    fn user(id: &str) -> UserRepresentation {
        UserRepresentation {
            id: id.into(),
            realm_id: R.into(),
            username: id.into(),
            ..Default::default()
        }
    }

    fn group(name: &str, org_id: Option<&str>, path: &str) -> GroupRepresentation {
        GroupRepresentation {
            realm_id: R.into(),
            name: name.into(),
            org_id: org_id.map(str::to_owned),
            path: path.into(),
            ..Default::default()
        }
    }

    /// An organization admin sees a person through a group of the organization: one the group
    /// names as its own, or one under the organization's path.
    #[tokio::test]
    async fn a_group_of_the_organization_makes_its_member_visible() {
        let store = MemoryStore::default();
        let hel: &[String] = &["hel".to_owned()];
        for (id, g) in [
            ("owned", group("a", Some("hel"), "/a")),
            ("pathed", group("b", Some("other"), "/hel/b")),
            ("outside", group("c", Some("other"), "/other/c")),
        ] {
            store.create_user(user(id)).await.expect("user");
            let g = store.upsert_group(g).await.expect("group");
            store.add_user_to_group(R, id, &g.id).await.expect("member");
        }
        store.create_user(user("alone")).await.expect("user");
        for (id, visible) in [
            ("owned", true),
            ("pathed", true),
            ("outside", false),
            ("alone", false),
        ] {
            let got = store.get_user_by_id(R, Some(hel), id).await.expect("read");
            assert_eq!(got.is_some(), visible, "{id}");
            assert!(
                store
                    .get_user_by_id(R, None, id)
                    .await
                    .expect("read")
                    .is_some()
            );
        }
    }

    /// One row of every entity set in `realm`, each id `{realm}-{kind}`; the realm itself is
    /// stored under the id `realm` and named `{realm}-name`.
    fn seed_every_set(st: &mut State, realm: &str, org: &str) {
        let now = TimestampTz::now();
        let r = || realm.to_owned();
        let id = |kind: &str| format!("{realm}-{kind}");
        st.realms.insert(
            r(),
            RealmConfig {
                id: r(),
                realm: id("name"),
                ..RealmConfig::default()
            },
        );
        st.organizations.push(Organization {
            id: id("org"),
            realm_id: r(),
            name: id("org"),
            alias: id("org"),
            enabled: true,
            description: String::new(),
            domains: vec![OrganizationDomain {
                id: id("dom"),
                realm_id: r(),
                org_id: id("org"),
                name: format!("{realm}.example"),
                verified: true,
                verification_token: None,
                challenge_hash: None,
                challenge_expires_at: None,
            }],
            attributes: HashMap::new(),
            managed_by: None,
        });
        st.memberships.push(Membership {
            id: id("ms"),
            realm_id: r(),
            org_id: id("org"),
            person_id: "p".into(),
            org_roles: Vec::new(),
            created_at: now,
        });
        // The same person id in every realm, each in its own organization.
        st.users.push(UserRepresentation {
            id: "p".into(),
            realm_id: r(),
            org_id: Some(org.into()),
            username: id("user"),
            ..UserRepresentation::default()
        });
        st.credentials.push(CredentialRecord {
            id: id("cred"),
            realm_id: r(),
            user_id: "p".into(),
            ..CredentialRecord::default()
        });
        st.groups.push(GroupRepresentation {
            id: id("group"),
            ..group("g", None, "/g")
        });
        st.groups.last_mut().expect("group").realm_id = r();
        st.group_members.push(GroupMemberRecord {
            id: id("gm"),
            realm_id: r(),
            org_id: None,
            group_id: id("group"),
            user_id: "p".into(),
        });
        st.roles.push(RoleRepresentation {
            id: id("role"),
            realm_id: r(),
            name: "viewer".into(),
            ..RoleRepresentation::default()
        });
        st.role_mappings.push(RoleMappingRecord {
            id: id("rm"),
            realm_id: r(),
            org_id: None,
            principal_kind: "user".into(),
            principal_id: "p".into(),
            role_id: id("role"),
            role_name: "viewer".into(),
            client_uuid: None,
        });
        let mapper = |kind: &str| ProtocolMapperRepresentation {
            id: id(kind),
            name: id(kind),
            ..ProtocolMapperRepresentation::default()
        };
        st.clients.push(ClientRepresentation {
            id: id("client"),
            realm_id: r(),
            client_id: id("client"),
            protocol_mappers: vec![mapper("client-mapper")],
            ..ClientRepresentation::default()
        });
        st.client_scopes.push(ClientScopeRepresentation {
            id: id("scope"),
            realm_id: r(),
            name: id("scope"),
            protocol_mappers: vec![mapper("scope-mapper")],
            ..ClientScopeRepresentation::default()
        });
        st.idps.push(IdentityProviderRepresentation {
            internal_id: id("idp"),
            realm_id: r(),
            alias: id("idp"),
            ..IdentityProviderRepresentation::default()
        });
        st.signing_keys.push(SigningKeyRecord {
            id: id("key"),
            realm_id: r(),
            kid: id("key"),
            algorithm: "ES256".into(),
            status: "active".into(),
            priority: 1,
            public_jwk_json: "{}".into(),
            encrypted_private_key: Vec::new(),
            created_at: now,
            activate_at: None,
            expires_at: None,
        });
        st.sso_sessions.push(SsoSession {
            id: id("sso"),
            realm_id: r(),
            org_id: None,
            handle_hash: id("h"),
            user_id: "p".into(),
            username: "p".into(),
            ip_address: String::new(),
            user_agent: String::new(),
            start: 0,
            last_access: 0,
            auth_time: 0,
            acr: "1".into(),
            amr: Vec::new(),
            clients: HashMap::new(),
            idle_expires_at: now,
            max_expires_at: now,
        });
        st.client_sessions.push(ClientSession {
            id: id("cs"),
            realm_id: r(),
            org_id: None,
            sso_session_id: id("sso"),
            client_id: id("client"),
            user_id: "p".into(),
            scopes: Vec::new(),
            created_at: now,
            last_access: now,
        });
        st.offline_sessions.push(OfflineSession {
            id: id("off"),
            realm_id: r(),
            org_id: None,
            user_id: "p".into(),
            client_id: id("client"),
            scopes: Vec::new(),
            created_at: now,
            last_access: now,
            idle_expires_at: now,
            max_expires_at: now,
        });
        st.consents.push(ConsentRecord {
            id: id("consent"),
            realm_id: r(),
            org_id: None,
            user_id: "p".into(),
            client_id: id("client"),
            granted_client_scopes: Vec::new(),
            created_date: 0,
            last_updated_date: 0,
        });
        st.required_actions.push(RequiredActionConfig {
            id: id("ra"),
            realm_id: r(),
            ..RequiredActionConfig::default()
        });
        st.brandings.push(BrandingBlock {
            id: id("brand"),
            realm_id: r(),
            ..BrandingBlock::default()
        });
        st.events.push(AuditEvent {
            id: id("event"),
            realm_id: r(),
            org_id: None,
            time: 0,
            event_type: "LOGIN".into(),
            client_id: None,
            user_id: None,
            session_id: None,
            ip_address: String::new(),
            error: None,
            details: std::collections::BTreeMap::new(),
        });
        st.admin_events.push(AdminEvent {
            id: id("admin-event"),
            realm_id: r(),
            org_id: None,
            time: 0,
            operation_type: "CREATE".into(),
            resource_type: "USER".into(),
            resource_path: "users/p".into(),
            actor_id: "a".into(),
            actor_client_id: None,
            representation: None,
        });
        st.webauthn_credentials.push(WebAuthnCredentialRecord {
            id: id("wa"),
            realm_id: r(),
            org_id: None,
            user_id: "p".into(),
            user_handle: id("uh"),
            credential_id: id("cid"),
            public_key_cose_json: "{}".into(),
            cose_alg: -7,
            sign_count: 0,
            uv_initialized: false,
            backup_eligible: false,
            backup_state: false,
            transports: Vec::new(),
            attestation_type: "none".into(),
            rp_id: "example.org".into(),
            user_label: None,
            created_at: now,
            last_used_at: None,
        });
        st.email_outbox.push(EmailOutboxRecord {
            id: id("mail"),
            realm_id: r(),
            org_id: None,
            user_id: "p".into(),
            to_address: "p@example.org".into(),
            locale: "en".into(),
            template_kind: "verify-email".into(),
            action_jti: id("jti"),
            actions: Vec::new(),
            client_id: None,
            redirect_uri: None,
            lifespan_secs: 60,
            issuer: "https://id.example.org".into(),
            status: "queued".into(),
            attempts: 0,
            last_error: None,
            next_attempt_at: now,
            created_at: now,
            sent_at: None,
        });
        st.idp_links.push(IdentityProviderLinkRecord {
            id: id("link"),
            realm_id: r(),
            org_id: None,
            user_id: "p".into(),
            idp_alias: id("idp"),
            external_user_id: id("ext"),
            external_username: None,
            created_at: now,
        });
    }

    /// Each entity set answers the rows of the realm asked for and none of another realm; a
    /// credential carries the organization of its own realm's person.
    #[test]
    fn every_entity_set_reads_only_its_realms_rows() {
        let store = MemoryStore::default();
        {
            let mut st = store.inner.write();
            seed_every_set(&mut st, "r", "hel");
            seed_every_set(&mut st, "q", "esp");
        }
        let st = store.inner.read();
        let ids = |realm: &str, set: &str| -> Vec<String> {
            let mut ids: Vec<String> = store
                .build_entity_rows(&st, realm, set)
                .iter()
                .map(|row| {
                    let key = row.get("id").or_else(|| row.get("internalId"));
                    key.and_then(Value::as_str).expect("key").to_owned()
                })
                .collect();
            ids.sort_unstable();
            ids
        };
        let service = odata_defs::kaisatsu_odata_service();
        assert_eq!(service.sets.len(), 26);
        for def in &service.sets {
            let want: Vec<String> = match def.set.as_str() {
                "Realms" => vec!["r".into()],
                "People" => vec!["p".into()],
                "ProtocolMappers" => vec!["r-client-mapper".into(), "r-scope-mapper".into()],
                set => {
                    let kind = match set {
                        "Organizations" => "org",
                        "OrganizationDomains" => "dom",
                        "Memberships" => "ms",
                        "Groups" => "group",
                        "GroupMembers" => "gm",
                        "Roles" => "role",
                        "RoleMappings" => "rm",
                        "Applications" => "client",
                        "ClientScopes" => "scope",
                        "IdentityProviders" => "idp",
                        "Credentials" => "cred",
                        "Sessions" => "sso",
                        "ClientSessions" => "cs",
                        "OfflineSessions" => "off",
                        "Consents" => "consent",
                        "Events" => "event",
                        "AdminEvents" => "admin-event",
                        "SigningKeys" => "key",
                        "RequiredActions" => "ra",
                        "BrandingBlocks" => "brand",
                        "WebAuthnCredentials" => "wa",
                        "EmailOutbox" => "mail",
                        "IdentityProviderLinks" => "link",
                        other => panic!("no expectation for {other}"),
                    };
                    vec![format!("r-{kind}")]
                }
            };
            assert_eq!(ids("r", &def.set), want, "{}", def.set);
        }
        assert_eq!(
            ids("r-name", "Realms"),
            ["r"],
            "a realm is found by its name too"
        );
        assert!(ids("nowhere", "Realms").is_empty());
        assert!(ids("r", "NoSuchSet").is_empty());
        let creds = store.build_entity_rows(&st, "q", "Credentials");
        assert_eq!(creds.len(), 1);
        assert_eq!(
            creds[0]["org_id"], "esp",
            "the organization of q's own person"
        );
    }

    fn org(realm: &str, id: &str, alias: &str) -> Organization {
        Organization {
            id: id.into(),
            realm_id: realm.into(),
            name: alias.into(),
            alias: alias.into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: HashMap::new(),
            managed_by: None,
        }
    }

    /// An organization is replaced by its id or its alias within its realm, found by either,
    /// and listed when either is allowed; the same names in another realm are another one.
    #[tokio::test]
    async fn organizations_are_kept_per_realm_by_id_or_alias() {
        let store = MemoryStore::default();
        for realm in [R, "q"] {
            store
                .upsert_organization(org(realm, "o1", "hel"))
                .await
                .expect("org");
        }
        store
            .upsert_organization(org(R, "o2", "hel"))
            .await
            .expect("same alias");
        store
            .upsert_organization(org(R, "o2", "helsinki"))
            .await
            .expect("same id");
        let ids = |realm: &'static str| {
            let store = &store;
            async move {
                store
                    .list_organizations(realm, None)
                    .await
                    .expect("list")
                    .into_iter()
                    .map(|o| (o.id, o.alias))
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(ids(R).await, [("o2".to_owned(), "helsinki".to_owned())]);
        assert_eq!(ids("q").await, [("o1".to_owned(), "hel".to_owned())]);
        for key in ["o2", "helsinki"] {
            let found = store.get_organization(R, key).await.expect("get");
            assert_eq!(found.map(|o| o.id).as_deref(), Some("o2"), "{key}");
        }
        assert!(
            store
                .get_organization(R, "o1")
                .await
                .expect("get")
                .is_none()
        );
        assert!(
            store
                .get_organization(R, "hel")
                .await
                .expect("get")
                .is_none()
        );
        assert!(
            store
                .get_organization("q", "hel")
                .await
                .expect("get")
                .is_some()
        );
        for allowed in ["o2", "helsinki"] {
            let only: &[String] = &[allowed.to_owned()];
            assert_eq!(
                store
                    .list_organizations(R, Some(only))
                    .await
                    .expect("list")
                    .len(),
                1,
                "allowed by {allowed}"
            );
        }
        let other: &[String] = &["o1".to_owned()];
        assert!(
            store
                .list_organizations(R, Some(other))
                .await
                .expect("list")
                .is_empty()
        );
    }

    fn membership(realm: &str, org_id: &str, person: &str, role: &str) -> Membership {
        Membership {
            id: format!("{realm}-{org_id}-{person}"),
            realm_id: realm.into(),
            org_id: org_id.into(),
            person_id: person.into(),
            org_roles: vec![role.into()],
            created_at: TimestampTz::now(),
        }
    }

    /// A membership is one per realm, organization and person: writing it again replaces that
    /// one only, and a person's list holds their own realm's memberships.
    #[tokio::test]
    async fn memberships_are_one_per_realm_organization_and_person() {
        let store = MemoryStore::default();
        for (realm, o, p) in [
            (R, "a", "p"),
            (R, "b", "p"),
            ("q", "a", "p"),
            (R, "a", "p2"),
        ] {
            store
                .upsert_membership(membership(realm, o, p, "member"))
                .await
                .expect("membership");
        }
        store
            .upsert_membership(membership(R, "a", "p", "org-admin"))
            .await
            .expect("again");
        let roles = |realm: &'static str, person: &'static str| {
            let store = &store;
            async move {
                let mut got: Vec<(String, Vec<String>)> = store
                    .list_memberships_for_person(realm, person)
                    .await
                    .expect("list")
                    .into_iter()
                    .map(|m| (m.org_id, m.org_roles))
                    .collect();
                got.sort();
                got
            }
        };
        assert_eq!(
            roles(R, "p").await,
            [
                ("a".to_owned(), vec!["org-admin".to_owned()]),
                ("b".to_owned(), vec!["member".to_owned()])
            ]
        );
        assert_eq!(
            roles("q", "p").await,
            [("a".to_owned(), vec!["member".to_owned()])]
        );
        assert_eq!(
            roles(R, "p2").await,
            [("a".to_owned(), vec!["member".to_owned()])]
        );
        assert!(roles("q", "p2").await.is_empty());
    }

    fn credential(realm: &str, id: &str, user: &str, kind: &str) -> CredentialRecord {
        CredentialRecord {
            id: id.into(),
            realm_id: realm.into(),
            user_id: user.into(),
            credential_type: kind.into(),
            secret_data: format!("{realm}-{id}"),
            ..CredentialRecord::default()
        }
    }

    /// A new password moves the person's old one to history, in their realm only; credentials
    /// are replaced by id within a realm, listed per person and deleted by person and id.
    #[tokio::test]
    async fn credentials_are_kept_per_realm_and_person_with_password_history() {
        let store = MemoryStore::default();
        for c in [
            credential(R, "pw1", "p", "password"),
            credential("q", "pw1", "p", "password"),
            credential(R, "otp", "p", "otp"),
            credential(R, "pw-b", "p2", "password"),
        ] {
            store.upsert_credential(c).await.expect("credential");
        }
        store
            .upsert_credential(credential(R, "pw2", "p", "password"))
            .await
            .expect("new password");
        let kinds = |realm: &'static str, user: &'static str| {
            let store = &store;
            async move {
                let mut got: Vec<(String, String)> = store
                    .list_credentials(realm, user)
                    .await
                    .expect("list")
                    .into_iter()
                    .map(|c| (c.credential_type, c.secret_data))
                    .collect();
                got.sort();
                got
            }
        };
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            kinds(R, "p").await,
            [
                pair("otp", "r-otp"),
                pair("password", "r-pw2"),
                pair("password-history", "r-pw1")
            ]
        );
        assert_eq!(kinds("q", "p").await, [pair("password", "q-pw1")]);
        assert_eq!(kinds(R, "p2").await, [pair("password", "r-pw-b")]);

        store
            .upsert_credential(credential("q", "otp", "p", "otp"))
            .await
            .expect("same id elsewhere");
        assert_eq!(kinds(R, "p").await.len(), 3, "q's otp leaves r's alone");
        assert!(
            !store
                .delete_credential(R, "p2", "otp")
                .await
                .expect("delete"),
            "not p2's"
        );
        assert!(
            !store
                .delete_credential("x", "p", "otp")
                .await
                .expect("delete")
        );
        assert!(
            store
                .delete_credential(R, "p", "otp")
                .await
                .expect("delete")
        );
        assert_eq!(
            kinds(R, "p").await,
            [pair("password", "r-pw2"), pair("password-history", "r-pw1")]
        );
        assert_eq!(
            kinds("q", "p").await,
            [pair("otp", "q-otp"), pair("password", "q-pw1")]
        );
    }

    /// A group under an organization's id or alias belongs to it, in that organization's
    /// realm only; an edit keeping its own path is no conflict, taking another's in the realm is.
    #[tokio::test]
    async fn groups_take_their_realms_organization_and_paths_are_unique_per_realm() {
        let store = MemoryStore::default();
        store
            .upsert_organization(org(R, "o9", "esp"))
            .await
            .expect("org");
        let put = |realm: &'static str, name: &'static str, path: &'static str| {
            let store = &store;
            async move {
                let mut g = group(name, None, path);
                g.realm_id = realm.into();
                store.upsert_group(g).await.expect("group")
            }
        };
        assert_eq!(put(R, "a", "/esp/a").await.org_id.as_deref(), Some("esp"));
        assert_eq!(put(R, "b", "/o9/b").await.org_id.as_deref(), Some("o9"));
        assert_eq!(put("q", "a", "/esp/a").await.org_id, None, "esp is r's");
        assert_eq!(put(R, "c", "/other/c").await.org_id, None);

        let x = put(R, "x", "/x").await;
        put(R, "y", "/y").await;
        put("q", "z", "/z").await;
        let edited = store
            .update_group(GroupRepresentation {
                attributes: HashMap::from([("k".to_owned(), vec!["edited".to_owned()])]),
                ..x.clone()
            })
            .await
            .expect("own path");
        assert_eq!(edited.path, "/x");
        let taken = store
            .update_group(GroupRepresentation {
                path: "/y".into(),
                ..x.clone()
            })
            .await;
        assert!(
            matches!(taken, Err(KaisatsuError::Conflict(_))),
            "{taken:?}"
        );
        let elsewhere = store
            .update_group(GroupRepresentation {
                path: "/z".into(),
                ..x
            })
            .await
            .expect("q's path is free in r");
        assert_eq!(elsewhere.path, "/z");
    }

    /// Deleting a person drops their credentials, memberships, sessions, passkeys, links and
    /// user role mappings, and nothing of another person or of the same id in another realm.
    #[tokio::test]
    async fn deleting_a_person_drops_their_rows_only() {
        let store = MemoryStore::default();
        let now = TimestampTz::now();
        let team = store
            .upsert_group(GroupRepresentation {
                id: "ada".into(),
                ..group("team", None, "/team")
            })
            .await
            .expect("group");
        for (realm, id) in [(R, "ada"), (R, "bob"), ("r2", "ada")] {
            store
                .create_user(UserRepresentation {
                    realm_id: realm.into(),
                    ..user(id)
                })
                .await
                .expect("person");
            store
                .upsert_credential(CredentialRecord {
                    id: format!("{realm}-{id}-pw"),
                    realm_id: realm.into(),
                    user_id: id.into(),
                    ..CredentialRecord::default()
                })
                .await
                .expect("credential");
            store
                .upsert_sso_session(SsoSession {
                    id: format!("{realm}-{id}-sso"),
                    realm_id: realm.into(),
                    org_id: None,
                    handle_hash: format!("{realm}-{id}-h"),
                    user_id: id.into(),
                    username: id.into(),
                    ip_address: "127.0.0.1".into(),
                    user_agent: "t".into(),
                    start: 1,
                    last_access: 1,
                    auth_time: 1,
                    acr: "1".into(),
                    amr: Vec::new(),
                    clients: HashMap::new(),
                    idle_expires_at: now + jiff::SignedDuration::from_hours(1),
                    max_expires_at: now + jiff::SignedDuration::from_hours(2),
                })
                .await
                .expect("session");
            store
                .insert_webauthn_credential(WebAuthnCredentialRecord {
                    id: format!("{realm}-{id}-wa"),
                    realm_id: realm.into(),
                    org_id: None,
                    user_id: id.into(),
                    user_handle: format!("{realm}-{id}-uh"),
                    credential_id: format!("{realm}-{id}-cid"),
                    public_key_cose_json: "{}".into(),
                    cose_alg: -7,
                    sign_count: 0,
                    uv_initialized: true,
                    backup_eligible: false,
                    backup_state: false,
                    transports: Vec::new(),
                    attestation_type: "none".into(),
                    rp_id: "example.org".into(),
                    user_label: None,
                    created_at: now,
                    last_used_at: None,
                })
                .await
                .expect("passkey");
            store
                .upsert_idp_link(IdentityProviderLinkRecord {
                    id: format!("{realm}-{id}-link"),
                    realm_id: realm.into(),
                    org_id: None,
                    user_id: id.into(),
                    idp_alias: "google".into(),
                    external_user_id: format!("{realm}-{id}-ext"),
                    external_username: None,
                    created_at: now,
                })
                .await
                .expect("link");
            store
                .add_role_mapping(RoleMappingRecord {
                    id: String::new(),
                    realm_id: realm.into(),
                    org_id: None,
                    principal_kind: "user".into(),
                    principal_id: id.into(),
                    role_id: "viewer".into(),
                    role_name: "viewer".into(),
                    client_uuid: None,
                })
                .await
                .expect("mapping");
            if realm == R {
                store
                    .add_user_to_group(R, id, &team.id)
                    .await
                    .expect("member");
            }
        }
        store
            .add_role_mapping(RoleMappingRecord {
                id: String::new(),
                realm_id: R.into(),
                org_id: None,
                principal_kind: "group".into(),
                principal_id: team.id.clone(),
                role_id: "viewer".into(),
                role_name: "viewer".into(),
                client_uuid: None,
            })
            .await
            .expect("group mapping");

        assert!(store.delete_user(R, None, "ada").await.expect("delete"));
        let counts = |realm: &'static str, id: &'static str| {
            let store = &store;
            async move {
                [
                    store.list_credentials(realm, id).await.expect("c").len(),
                    store.list_user_groups(realm, id).await.expect("g").len(),
                    store
                        .list_user_sso_sessions(realm, id)
                        .await
                        .expect("s")
                        .len(),
                    store
                        .list_webauthn_credentials(realm, id)
                        .await
                        .expect("w")
                        .len(),
                    store
                        .list_idp_links_for_user(realm, id)
                        .await
                        .expect("l")
                        .len(),
                    store
                        .list_role_mappings_for_principal(realm, "user", id, None)
                        .await
                        .expect("m")
                        .len(),
                ]
            }
        };
        assert_eq!(counts(R, "ada").await, [0, 0, 0, 0, 0, 0], "ada is gone");
        assert_eq!(counts(R, "bob").await, [1, 1, 1, 1, 1, 1], "bob keeps his");
        assert_eq!(
            counts("r2", "ada").await,
            [1, 0, 1, 1, 1, 1],
            "ada of r2 keeps hers"
        );
        let kept = store
            .list_role_mappings_for_principal(R, "group", &team.id, None)
            .await
            .expect("group mappings");
        assert_eq!(kept.len(), 1, "the group's own mapping stays");
    }

    #[tokio::test]
    async fn deleting_a_group_drops_its_role_mappings_and_keeps_the_others() {
        let store = MemoryStore::default();
        let (kept, gone) = (
            store
                .upsert_group(group("kept", None, ""))
                .await
                .expect("group"),
            store
                .upsert_group(group("gone", None, ""))
                .await
                .expect("group"),
        );
        let role = store
            .upsert_role(RoleRepresentation {
                realm_id: R.into(),
                name: "viewer".into(),
                ..Default::default()
            })
            .await
            .expect("role");
        for g in [&kept, &gone] {
            store
                .add_role_mapping(RoleMappingRecord {
                    id: String::new(),
                    realm_id: R.into(),
                    org_id: None,
                    principal_kind: "group".into(),
                    principal_id: g.id.clone(),
                    role_id: role.id.clone(),
                    role_name: role.name.clone(),
                    client_uuid: None,
                })
                .await
                .expect("mapping");
        }
        assert!(store.delete_group(R, None, &gone.id).await.expect("delete"));
        let mappings = |id: String| {
            let store = &store;
            async move {
                store
                    .list_role_mappings_for_principal(R, "group", &id, None)
                    .await
                    .expect("mappings")
                    .len()
            }
        };
        assert_eq!(mappings(gone.id.clone()).await, 0);
        assert_eq!(mappings(kept.id.clone()).await, 1);
        assert!(!store.delete_group(R, None, &gone.id).await.expect("again"));
    }

    fn client(client_id: &str) -> ClientRepresentation {
        ClientRepresentation {
            realm_id: R.into(),
            client_id: client_id.into(),
            ..Default::default()
        }
    }

    fn consent(user_id: &str, client_id: &str) -> ConsentRecord {
        ConsentRecord {
            id: String::new(),
            realm_id: R.into(),
            org_id: None,
            user_id: user_id.into(),
            client_id: client_id.into(),
            granted_client_scopes: vec!["profile".into()],
            created_date: 1,
            last_updated_date: 1,
        }
    }

    #[tokio::test]
    async fn renaming_a_client_drops_the_consents_given_to_its_old_id() {
        let store = MemoryStore::default();
        let mut app = store.upsert_client(client("app")).await.expect("client");
        store.upsert_client(client("other")).await.expect("client");
        store.create_user(user("u1")).await.expect("user");
        for c in ["app", "other"] {
            store
                .upsert_consent(consent("u1", c))
                .await
                .expect("consent");
        }
        app.client_id = "app2".into();
        let renamed = store.upsert_client(app).await.expect("renamed");
        assert_eq!(renamed.client_id, "app2");
        let left: Vec<String> = store
            .list_consents(R, "u1")
            .await
            .expect("consents")
            .into_iter()
            .map(|c| c.client_id)
            .collect();
        assert_eq!(left, ["other"]);
    }

    #[tokio::test]
    async fn deleting_a_client_drops_its_role_mappings_only() {
        let store = MemoryStore::default();
        let (app, other) = (
            store.upsert_client(client("app")).await.expect("client"),
            store.upsert_client(client("other")).await.expect("client"),
        );
        store.create_user(user("u1")).await.expect("user");
        for c in [&app, &other] {
            let role = store
                .upsert_role(RoleRepresentation {
                    realm_id: R.into(),
                    name: "reader".into(),
                    client_role: true,
                    container_id: Some(c.id.clone()),
                    ..Default::default()
                })
                .await
                .expect("role");
            store
                .add_role_mapping(RoleMappingRecord {
                    id: String::new(),
                    realm_id: R.into(),
                    org_id: None,
                    principal_kind: "user".into(),
                    principal_id: "u1".into(),
                    role_id: role.id.clone(),
                    role_name: role.name.clone(),
                    client_uuid: Some(c.id.clone()),
                })
                .await
                .expect("mapping");
        }
        assert!(store.delete_client(R, None, &app.id).await.expect("delete"));
        for (c, left) in [(&app, 0), (&other, 1)] {
            let mappings = store
                .list_role_mappings_for_principal(R, "user", "u1", Some(&c.id))
                .await
                .expect("mappings");
            assert_eq!(mappings.len(), left, "{}", c.client_id);
        }
    }

    #[tokio::test]
    async fn deleting_a_client_drops_the_consents_given_to_it_only() {
        let store = MemoryStore::default();
        let app = store.upsert_client(client("app")).await.expect("client");
        store.upsert_client(client("other")).await.expect("client");
        store.create_user(user("u1")).await.expect("user");
        for c in ["app", "other"] {
            store
                .upsert_consent(consent("u1", c))
                .await
                .expect("consent");
        }
        assert!(store.delete_client(R, None, &app.id).await.expect("delete"));
        let left: Vec<String> = store
            .list_consents(R, "u1")
            .await
            .expect("consents")
            .into_iter()
            .map(|c| c.client_id)
            .collect();
        assert_eq!(left, ["other"]);
    }

    fn mapping(
        user_id: &str,
        role: &RoleRepresentation,
        client_uuid: Option<&str>,
    ) -> RoleMappingRecord {
        RoleMappingRecord {
            id: String::new(),
            realm_id: R.into(),
            org_id: None,
            principal_kind: "user".into(),
            principal_id: user_id.into(),
            role_id: role.id.clone(),
            role_name: role.name.clone(),
            client_uuid: client_uuid.map(str::to_owned),
        }
    }

    /// A client role goes from the people holding it; a person holding only another client's
    /// roles keeps them.
    #[tokio::test]
    async fn deleting_a_client_role_drops_it_from_its_holders_only() {
        let store = MemoryStore::default();
        let (app, other) = (
            store.upsert_client(client("app")).await.expect("client"),
            store.upsert_client(client("other")).await.expect("client"),
        );
        let role = |c: &ClientRepresentation| RoleRepresentation {
            realm_id: R.into(),
            name: "reader".into(),
            client_role: true,
            container_id: Some(c.id.clone()),
            ..Default::default()
        };
        let (app_role, other_role) = (
            store.upsert_role(role(&app)).await.expect("role"),
            store.upsert_role(role(&other)).await.expect("role"),
        );
        for id in ["holder", "elsewhere"] {
            store.create_user(user(id)).await.expect("user");
        }
        store
            .add_role_mapping(mapping("holder", &app_role, Some(&app.id)))
            .await
            .expect("map");
        store
            .add_role_mapping(mapping("elsewhere", &other_role, Some(&other.id)))
            .await
            .expect("map");
        assert!(
            store
                .delete_role(R, Some(&app.id), "reader")
                .await
                .expect("delete")
        );
        let roles = |id: &'static str| {
            let store = &store;
            async move {
                store
                    .get_user_by_id(R, None, id)
                    .await
                    .expect("read")
                    .expect("person")
                    .client_roles
            }
        };
        assert!(roles("holder").await.get("app").is_none_or(Vec::is_empty));
        assert_eq!(
            roles("elsewhere").await.get("other"),
            Some(&vec!["reader".to_owned()])
        );
    }

    /// A rename may take a client id another realm uses; it renames this client's service
    /// account and no other person.
    #[tokio::test]
    async fn a_client_rename_is_checked_and_applied_in_its_own_realm() {
        let store = MemoryStore::default();
        let mut app = store.upsert_client(client("app")).await.expect("client");
        store
            .upsert_client(ClientRepresentation {
                realm_id: "q".into(),
                ..client("fresh")
            })
            .await
            .expect("q's client");
        store
            .create_user(UserRepresentation {
                username: "service-account-app".into(),
                service_account_client_id: Some("app".into()),
                ..user("sa")
            })
            .await
            .expect("service account");
        store.create_user(user("ada")).await.expect("person");
        store
            .create_user(UserRepresentation {
                realm_id: "q".into(),
                service_account_client_id: Some("app".into()),
                ..user("q-sa")
            })
            .await
            .expect("q's account");
        app.client_id = "fresh".into();
        let renamed = store.upsert_client(app).await.expect("q's id is free in r");
        assert_eq!(renamed.client_id, "fresh");
        let name = |realm: &'static str, id: &'static str| {
            let store = &store;
            async move {
                let u = store
                    .get_user_by_id(realm, None, id)
                    .await
                    .expect("read")
                    .expect("person");
                (u.username, u.service_account_client_id)
            }
        };
        assert_eq!(
            name(R, "sa").await,
            ("service-account-fresh".to_owned(), Some("fresh".to_owned()))
        );
        assert_eq!(name(R, "ada").await, ("ada".to_owned(), None));
        assert_eq!(
            name("q", "q-sa").await,
            ("q-sa".to_owned(), Some("app".to_owned()))
        );
    }

    /// A mapping written again replaces only itself, a removal takes only the named one, and a
    /// role delete takes only that role's mappings: kind, holder, client, name and realm each
    /// tell mappings apart.
    #[tokio::test]
    async fn role_mappings_are_told_apart_by_every_part() {
        let store = MemoryStore::default();
        let (app, other) = (
            store.upsert_client(client("app")).await.expect("client"),
            store.upsert_client(client("other")).await.expect("client"),
        );
        let mut g = group("ada", None, "/ada");
        g.id = "ada".into();
        store.upsert_group(g).await.expect("group");
        for realm in [R, "q"] {
            for id in ["ada", "bob"] {
                store
                    .create_user(UserRepresentation {
                        realm_id: realm.into(),
                        ..user(id)
                    })
                    .await
                    .expect("person");
            }
        }
        let m = |realm: &str, kind: &str, who: &str, uuid: Option<&str>, name: &str| {
            RoleMappingRecord {
                id: String::new(),
                realm_id: realm.into(),
                org_id: None,
                principal_kind: kind.into(),
                principal_id: who.into(),
                role_id: name.into(),
                role_name: name.into(),
                client_uuid: uuid.map(str::to_owned),
            }
        };
        let all = [
            m(R, "user", "ada", None, "reader"),
            m(R, "user", "ada", None, "writer"),
            m(R, "user", "ada", Some(&app.id), "reader"),
            m(R, "user", "ada", Some(&other.id), "reader"),
            m(R, "user", "bob", None, "reader"),
            m(R, "group", "ada", None, "reader"),
            m("q", "user", "ada", None, "reader"),
        ];
        for mapping in all.iter().cloned() {
            store.add_role_mapping(mapping).await.expect("map");
        }
        store
            .add_role_mapping(m(R, "user", "ada", None, "reader"))
            .await
            .expect("again");
        let held =
            |realm: &'static str, kind: &'static str, who: &'static str, uuid: Option<String>| {
                let store = &store;
                async move {
                    let mut got: Vec<String> = store
                        .list_role_mappings_for_principal(realm, kind, who, uuid.as_deref())
                        .await
                        .expect("mappings")
                        .into_iter()
                        .map(|rm| rm.role_name)
                        .collect();
                    got.sort();
                    got
                }
            };
        let (a, o) = (Some(app.id.clone()), Some(other.id.clone()));
        assert_eq!(
            held(R, "user", "ada", None).await,
            ["reader", "writer"],
            "ada's once"
        );
        assert_eq!(held(R, "user", "ada", a.clone()).await, ["reader"]);
        assert_eq!(held(R, "user", "ada", o.clone()).await, ["reader"]);
        assert_eq!(held(R, "user", "bob", None).await, ["reader"]);
        assert_eq!(held(R, "group", "ada", None).await, ["reader"]);
        assert_eq!(held("q", "user", "ada", None).await, ["reader"]);

        store
            .remove_role_mapping(R, "user", "ada", Some(&other.id), "reader")
            .await
            .expect("remove");
        assert!(held(R, "user", "ada", o.clone()).await.is_empty());
        assert_eq!(held(R, "user", "ada", a.clone()).await, ["reader"]);
        let person = store
            .get_user_by_id(R, None, "ada")
            .await
            .expect("read")
            .expect("ada");
        assert_eq!(
            person.client_roles.get("app"),
            Some(&vec!["reader".to_owned()])
        );
        assert!(person.client_roles.get("other").is_none_or(Vec::is_empty));
        store
            .remove_role_mapping(R, "user", "ada", None, "writer")
            .await
            .expect("remove");
        assert_eq!(held(R, "user", "ada", None).await, ["reader"]);
        assert_eq!(held(R, "user", "ada", a.clone()).await, ["reader"]);
        assert_eq!(held(R, "group", "ada", None).await, ["reader"]);
        assert_eq!(held(R, "user", "bob", None).await, ["reader"]);
        assert_eq!(held("q", "user", "ada", None).await, ["reader"]);

        store
            .upsert_role(RoleRepresentation {
                realm_id: R.into(),
                name: "reader".into(),
                ..Default::default()
            })
            .await
            .expect("role");
        assert!(store.delete_role(R, None, "reader").await.expect("delete"));
        assert!(
            held(R, "user", "bob", None).await.is_empty(),
            "the realm role is gone"
        );
        assert!(held(R, "group", "ada", None).await.is_empty());
        assert_eq!(
            held(R, "user", "ada", a).await,
            ["reader"],
            "the client role stays"
        );
        assert_eq!(
            held("q", "user", "ada", None).await,
            ["reader"],
            "q keeps its role"
        );
    }

    /// Deleting the second client's role takes it from that client's holders, not the first's.
    #[tokio::test]
    async fn deleting_a_role_of_another_client_leaves_the_first_clients_holders() {
        let store = MemoryStore::default();
        let (app, other) = (
            store.upsert_client(client("app")).await.expect("client"),
            store.upsert_client(client("other")).await.expect("client"),
        );
        store.create_user(user("both")).await.expect("user");
        for c in [&app, &other] {
            let role = store
                .upsert_role(RoleRepresentation {
                    realm_id: R.into(),
                    name: "reader".into(),
                    client_role: true,
                    container_id: Some(c.id.clone()),
                    ..Default::default()
                })
                .await
                .expect("role");
            store
                .add_role_mapping(mapping("both", &role, Some(&c.id)))
                .await
                .expect("map");
        }
        assert!(
            store
                .delete_role(R, Some(&other.id), "reader")
                .await
                .expect("delete")
        );
        let held = store
            .get_user_by_id(R, None, "both")
            .await
            .expect("read")
            .expect("person")
            .client_roles;
        assert_eq!(held.get("app"), Some(&vec!["reader".to_owned()]));
        assert!(held.get("other").is_none_or(Vec::is_empty));
    }

    /// A role is one per realm, container and name: written again it keeps its id; a realm role
    /// and a client role of one name, or one name in two realms, are separate roles.
    #[tokio::test]
    async fn roles_are_one_per_realm_container_and_name() {
        let store = MemoryStore::default();
        let app = store.upsert_client(client("app")).await.expect("client");
        let role = |realm: &str, container: Option<&str>| RoleRepresentation {
            realm_id: realm.into(),
            name: "reader".into(),
            client_role: container.is_some(),
            container_id: container.map(str::to_owned),
            ..Default::default()
        };
        let realm_role = store.upsert_role(role(R, None)).await.expect("role");
        let client_role = store
            .upsert_role(role(R, Some(&app.id)))
            .await
            .expect("role");
        let q_role = store.upsert_role(role("q", None)).await.expect("role");
        assert_ne!(realm_role.id, client_role.id);
        assert_ne!(realm_role.id, q_role.id);
        let again = store
            .upsert_role(RoleRepresentation {
                description: Some("again".into()),
                ..role(R, None)
            })
            .await
            .expect("again");
        assert_eq!(again.id, realm_role.id, "the same role keeps its id");
        let realm_roles = store.list_roles(R, None).await.expect("realm roles");
        assert_eq!(realm_roles.len(), 1);
        assert_eq!(realm_roles[0].description.as_deref(), Some("again"));
        let client_roles = store
            .list_roles(R, Some(&app.id))
            .await
            .expect("client roles");
        assert_eq!(client_roles.len(), 1);
        assert_eq!(client_roles[0].id, client_role.id);
        assert_eq!(store.list_roles("q", None).await.expect("q").len(), 1);
        assert!(
            store
                .list_roles("q", Some(&app.id))
                .await
                .expect("q")
                .is_empty()
        );
    }

    /// Membership is per realm: a group or person of another realm is not found, a second add
    /// is one membership, and a removal or a group delete takes only that membership.
    #[tokio::test]
    async fn group_membership_is_per_realm_group_and_person() {
        let store = MemoryStore::default();
        for realm in [R, "q"] {
            for id in ["ada", "bob"] {
                store
                    .create_user(UserRepresentation {
                        realm_id: realm.into(),
                        ..user(id)
                    })
                    .await
                    .expect("person");
            }
            for name in ["one", "two"] {
                let mut g = group(name, None, &format!("/{name}"));
                g.realm_id = realm.into();
                g.id = name.into();
                store.upsert_group(g).await.expect("group");
            }
        }
        store.create_user(user("only-r")).await.expect("person");
        let mut only = group("only", None, "/only");
        only.id = "only".into();
        store.upsert_group(only).await.expect("group");
        let missing = store.add_user_to_group("q", "ada", "only").await;
        assert!(
            matches!(missing, Err(KaisatsuError::NotFound(_))),
            "{missing:?}"
        );
        let nobody = store.add_user_to_group("q", "only-r", "one").await;
        assert!(
            matches!(nobody, Err(KaisatsuError::NotFound(_))),
            "{nobody:?}"
        );
        for (realm, person, g) in [
            (R, "ada", "one"),
            (R, "ada", "one"),
            (R, "ada", "two"),
            (R, "bob", "one"),
            ("q", "ada", "one"),
        ] {
            store
                .add_user_to_group(realm, person, g)
                .await
                .expect("member");
        }
        let members = |realm: &'static str, g: &'static str| {
            let store = &store;
            async move {
                let mut ids: Vec<String> = store
                    .list_group_members(realm, None, g, 0, 100)
                    .await
                    .expect("members")
                    .into_iter()
                    .map(|u| u.id)
                    .collect();
                ids.sort();
                ids
            }
        };
        assert_eq!(members(R, "one").await, ["ada", "bob"], "ada once");
        store
            .remove_user_from_group(R, "ada", "two")
            .await
            .expect("remove");
        store
            .remove_user_from_group(R, "bob", "two")
            .await
            .expect("remove nothing");
        assert!(members(R, "two").await.is_empty());
        assert_eq!(members(R, "one").await, ["ada", "bob"]);
        assert_eq!(members("q", "one").await, ["ada"]);
        store
            .add_user_to_group(R, "ada", "two")
            .await
            .expect("member again");
        assert!(store.delete_group(R, None, "one").await.expect("delete"));
        assert_eq!(
            members(R, "two").await,
            ["ada"],
            "another group keeps its members"
        );
        assert_eq!(
            members("q", "one").await,
            ["ada"],
            "q's group keeps its members"
        );
        assert!(
            store
                .get_group("q", None, "one")
                .await
                .expect("get")
                .is_some()
        );
    }

    /// A role mapped twice is held once; removing a client role from a person who holds none of
    /// that client changes nothing, and a realm role goes on its own.
    #[tokio::test]
    async fn role_mappings_are_held_once_and_removed_by_their_kind() {
        let store = MemoryStore::default();
        let app = store.upsert_client(client("app")).await.expect("client");
        let realm_role = store
            .upsert_role(RoleRepresentation {
                realm_id: R.into(),
                name: "viewer".into(),
                ..Default::default()
            })
            .await
            .expect("role");
        let mut u = user("u1");
        u.realm_roles = vec!["viewer".into()];
        store.create_user(u).await.expect("user");
        store
            .add_role_mapping(mapping("u1", &realm_role, None))
            .await
            .expect("map");
        let held = store
            .get_user_by_id(R, None, "u1")
            .await
            .expect("read")
            .expect("person");
        assert_eq!(held.realm_roles, ["viewer"], "held once");
        store
            .remove_role_mapping(R, "user", "u1", Some(&app.id), "reader")
            .await
            .expect("no client role held");
        let held = store
            .get_user_by_id(R, None, "u1")
            .await
            .expect("read")
            .expect("person");
        assert_eq!(held.realm_roles, ["viewer"]);
        assert!(held.client_roles.is_empty());
        store
            .remove_role_mapping(R, "user", "u1", None, "viewer")
            .await
            .expect("remove");
        let held = store
            .get_user_by_id(R, None, "u1")
            .await
            .expect("read")
            .expect("person");
        assert!(held.realm_roles.is_empty());
    }

    #[tokio::test]
    async fn an_unknown_person_or_device_code_changes_nothing() {
        let store = MemoryStore::default();
        assert!(!store.delete_user(R, None, "nobody").await.expect("delete"));
        let now = TimestampTz::now();
        let code = DeviceCodeRecord {
            device_code_hash: "d".into(),
            user_code_hash: "u".into(),
            realm_id: R.into(),
            client_id: "tv".into(),
            scope: "openid".into(),
            mapper_scopes: Vec::new(),
            status: DeviceStatus::Approved,
            user_id: Some("u1".into()),
            sso_session_id: None,
            acr: "1".into(),
            amr: Vec::new(),
            auth_time: 0,
            interval_secs: 5,
            last_poll_at: None,
            consumed: false,
            created_at: now,
            expires_at: now,
        };
        assert!(!store.decide_device_code(code).await.expect("decide"));
    }

    /// The e-mail filter matches a part of the address, or the whole of it when exact, in any case.
    #[tokio::test]
    async fn people_are_listed_by_part_or_whole_of_their_email() {
        let store = MemoryStore::default();
        for (id, email) in [
            ("ada", Some("Ada@City.example")),
            ("bob", Some("bob@town.example")),
            ("cy", None),
        ] {
            let mut u = user(id);
            u.email = email.map(str::to_owned);
            store.create_user(u).await.expect("user");
        }
        let ids = |email: &'static str, exact: bool| {
            let store = &store;
            async move {
                store
                    .list_users(R, None, None, Some(email), None, exact, 0, 10)
                    .await
                    .expect("list")
                    .into_iter()
                    .map(|u| u.id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(ids("city", false).await, ["ada"]);
        assert_eq!(ids("ada@city.example", true).await, ["ada"]);
        assert!(
            ids("ada@city", true).await.is_empty(),
            "exact is the whole address"
        );
        assert!(ids("nowhere", false).await.is_empty());
    }

    /// A sub-group keeps the path and Organization it names, and takes its parent's otherwise;
    /// moving a group onto another group's path is refused.
    #[tokio::test]
    async fn sub_groups_keep_what_they_name_and_a_taken_path_is_refused() {
        let store = MemoryStore::default();
        let mut parent = group("staff", Some("hel"), "/staff");
        parent.sub_groups = vec![
            group("named", Some("zil"), "/elsewhere/named"),
            group("plain", None, ""),
        ];
        let parent = store.upsert_group(parent).await.expect("group");
        let all = store.inner.read().groups.clone();
        let find = |name: &str| all.iter().find(|g| g.name == name).cloned().expect(name);
        let (named, plain) = (find("named"), find("plain"));
        assert_eq!(
            (named.path.as_str(), named.org_id.as_deref()),
            ("/elsewhere/named", Some("zil"))
        );
        assert_eq!(
            (plain.path.as_str(), plain.org_id.as_deref()),
            ("/staff/plain", Some("hel"))
        );
        assert_eq!(plain.parent_id.as_deref(), Some(parent.id.as_str()));
        let mut moved = plain.clone();
        moved.path = "/elsewhere/named".into();
        assert!(matches!(
            store.update_group(moved).await,
            Err(KaisatsuError::Conflict(_))
        ));
        let mut missing = plain;
        missing.id = "nope".into();
        assert!(matches!(
            store.update_group(missing).await,
            Err(KaisatsuError::NotFound(_))
        ));
    }

    /// An outbox row keeps the id it was given; only a row without one gets a fresh id.
    #[tokio::test]
    async fn an_outbox_row_keeps_its_own_id_and_a_sent_row_is_never_claimed() {
        let store = MemoryStore::default();
        store.create_user(user("u1")).await.expect("user");
        let row = |id: &str, status: &str| EmailOutboxRecord {
            id: id.into(),
            realm_id: R.into(),
            org_id: None,
            user_id: "u1".into(),
            to_address: "u1@example.test".into(),
            locale: "en".into(),
            template_kind: "EXECUTE_ACTIONS".into(),
            action_jti: format!("jti-{id}"),
            actions: vec!["UPDATE_PASSWORD".into()],
            client_id: None,
            redirect_uri: None,
            lifespan_secs: 900,
            issuer: "https://idm.example.test/realms/r".into(),
            status: status.into(),
            attempts: 0,
            last_error: None,
            next_attempt_at: TimestampTz::now() - SignedDuration::from_secs(1),
            created_at: TimestampTz::now(),
            sent_at: None,
        };
        store
            .enqueue_outbox(row("kept-id", "pending"))
            .await
            .expect("enqueue");
        store
            .enqueue_outbox(row("sent-id", "sent"))
            .await
            .expect("enqueue");
        store
            .update_user_with_outbox(user("u1"), row("with-user", "pending"))
            .await
            .expect("update");
        let ids: Vec<String> = store
            .inner
            .read()
            .email_outbox
            .iter()
            .map(|o| o.id.clone())
            .collect();
        assert_eq!(ids, ["kept-id", "sent-id", "with-user"]);
        let claimed = store
            .claim_outbox_batch(R, 10, std::time::Duration::from_secs(60), 3)
            .await
            .expect("claim");
        let claimed: Vec<&str> = claimed.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(claimed, ["kept-id", "with-user"], "a sent row stays sent");
    }

    /// A row handed over without an id gets a fresh one, on its own and beside a person's update.
    #[tokio::test]
    async fn an_outbox_row_without_an_id_gets_a_fresh_one() {
        let store = MemoryStore::default();
        store.create_user(user("u1")).await.expect("user");
        let row = EmailOutboxRecord {
            id: String::new(),
            realm_id: R.into(),
            org_id: None,
            user_id: "u1".into(),
            to_address: "u1@example.test".into(),
            locale: "en".into(),
            template_kind: "VERIFY_EMAIL".into(),
            action_jti: "jti".into(),
            actions: Vec::new(),
            client_id: None,
            redirect_uri: None,
            lifespan_secs: 900,
            issuer: "https://idm.example.test/realms/r".into(),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            next_attempt_at: TimestampTz::now(),
            created_at: TimestampTz::now(),
            sent_at: None,
        };
        store.enqueue_outbox(row.clone()).await.expect("enqueue");
        store
            .update_user_with_outbox(user("u1"), row)
            .await
            .expect("update");
        let ids: Vec<String> = store
            .inner
            .read()
            .email_outbox
            .iter()
            .map(|o| o.id.clone())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(
            ids.iter().all(|id| uuid::Uuid::parse_str(id).is_ok()),
            "{ids:?}"
        );
        assert_ne!(ids[0], ids[1]);
    }

    /// A role mapping for a person who is not there is refused, removing one from nobody changes
    /// nothing, and deleting a role whose client is gone touches no one else.
    #[tokio::test]
    async fn a_mapping_for_a_missing_holder_or_client_changes_no_one() {
        let store = MemoryStore::default();
        let realm_role = store
            .upsert_role(RoleRepresentation {
                realm_id: R.into(),
                name: "viewer".into(),
                ..Default::default()
            })
            .await
            .expect("role");
        let mut u = user("u1");
        u.realm_roles = vec!["viewer".into()];
        store.create_user(u).await.expect("user");
        assert_eq!(
            store
                .add_role_mapping(mapping("ghost", &realm_role, None))
                .await,
            Err(KaisatsuError::NotFound("user not found".into()))
        );
        store
            .remove_role_mapping(R, "user", "ghost", None, "viewer")
            .await
            .expect("removing it from nobody");
        let orphan = store
            .upsert_role(RoleRepresentation {
                realm_id: R.into(),
                name: "viewer".into(),
                container_id: Some("gone-client".into()),
                client_role: true,
                ..Default::default()
            })
            .await
            .expect("a role of a client that is gone");
        assert!(
            store
                .delete_role(R, orphan.container_id.as_deref(), "viewer")
                .await
                .expect("delete")
        );
        let held = store
            .get_user_by_id(R, None, "u1")
            .await
            .expect("read")
            .expect("person");
        assert_eq!(held.realm_roles, ["viewer"]);
        assert!(held.client_roles.is_empty());
    }

    /// A set the store keeps no rows of answers no row.
    #[test]
    fn a_set_without_rows_answers_none() {
        let store = MemoryStore::default();
        let st = store.inner.read();
        assert!(store.build_entity_rows(&st, R, "Nope").is_empty());
    }
}
