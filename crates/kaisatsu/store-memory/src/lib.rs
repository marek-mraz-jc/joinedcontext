#![forbid(unsafe_code)]
//! In-memory implementation of [`kaisatsu_core::Store`] with `odata-table::MemoryTable` backing
//! every entity set and enforcing the same organization isolation as PostgreSQL RLS (W2, W4).

use chrono::{Duration, Utc};
use kaisatsu_core::store::StoreFuture;
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, KeyManager};
use odata_table::{Answer, MemoryTable, QueryOptions, TableInfo, query};
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

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
        for gm in st
            .group_members
            .iter()
            .filter(|gm| gm.realm_id == realm_id && gm.user_id == user.id)
        {
            if let Some(g) = st
                .groups
                .iter()
                .find(|g| g.realm_id == realm_id && g.id == gm.group_id)
            {
                if g.org_id.as_ref().is_some_and(|o| allowed.contains(o)) {
                    return true;
                }
                if let Some(root) = g.path.strip_prefix('/').and_then(|p| p.split('/').next())
                    && allowed.iter().any(|a| a == root)
                {
                    return true;
                }
            }
        }
        false
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
                        .find(|u| u.id == c.user_id)
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

    pub fn csdl_model(&self) -> Arc<odata_csdl::Model> {
        self.csdl_model.clone()
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
            if user.created_timestamp.is_none() {
                user.created_timestamp = Some(Utc::now().timestamp_millis());
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
            st.users[pos] = user.clone();
            Ok(user)
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
            out.sort_by(|a, b| a.username.cmp(&b.username));
            Ok(out.into_iter().skip(first).take(max.max(1)).collect())
        })
    }

    fn upsert_credential<'a>(&'a self, mut cred: CredentialRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if cred.id.is_empty() {
                cred.id = uuid::Uuid::now_v7().to_string();
            }
            if cred.created_date == 0 {
                cred.created_date = Utc::now().timestamp_millis();
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
            let removed = st.groups.remove(pos);
            st.group_members
                .retain(|gm| !(gm.realm_id == realm_id && gm.group_id == group_id));
            for u in st.users.iter_mut().filter(|u| u.realm_id == realm_id) {
                u.groups
                    .retain(|p| p != &removed.path && p != &removed.name);
            }
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
            if let Some(u) = st
                .users
                .iter_mut()
                .find(|u| u.realm_id == realm_id && u.id == user_id)
                && !u.groups.contains(&group.path)
            {
                u.groups.push(group.path);
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
            let group_path = st
                .groups
                .iter()
                .find(|g| g.realm_id == realm_id && g.id == group_id)
                .map(|g| g.path.clone());
            st.group_members.retain(|gm| {
                !(gm.realm_id == realm_id && gm.user_id == user_id && gm.group_id == group_id)
            });
            if let Some(path) = group_path
                && let Some(u) = st
                    .users
                    .iter_mut()
                    .find(|u| u.realm_id == realm_id && u.id == user_id)
            {
                u.groups.retain(|p| p != &path);
            }
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
            let user_paths: Vec<String> = st
                .users
                .iter()
                .find(|u| u.realm_id == realm_id && u.id == user_id)
                .map(|u| u.groups.clone())
                .unwrap_or_default();
            let mut groups: Vec<GroupRepresentation> = st
                .groups
                .iter()
                .filter(|g| {
                    g.realm_id == realm_id
                        && (member_group_ids.contains(&g.id.as_str())
                            || user_paths.contains(&g.path)
                            || user_paths.contains(&g.name))
                })
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
            let member_ids: Vec<&str> = st
                .group_members
                .iter()
                .filter(|gm| gm.realm_id == realm_id && gm.group_id == group_id)
                .map(|gm| gm.user_id.as_str())
                .collect();
            let out: Vec<UserRepresentation> = st
                .users
                .iter()
                .filter(|u| {
                    u.realm_id == realm_id
                        && (member_ids.contains(&u.id.as_str()) || u.groups.contains(&group.path))
                })
                .take(max.max(1))
                .cloned()
                .collect();
            Ok(out)
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
            st.role_mappings.retain(|rm| {
                !(rm.realm_id == realm_id
                    && rm.role_name == role_name
                    && rm.client_uuid.as_deref() == container_id)
            });
            Ok(st.roles.len() < before)
        })
    }

    fn add_role_mapping<'a>(&'a self, mut mapping: RoleMappingRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if mapping.id.is_empty() {
                mapping.id = uuid::Uuid::now_v7().to_string();
            }
            let mut st = self.inner.write();
            st.role_mappings.retain(|rm| {
                !(rm.realm_id == mapping.realm_id
                    && rm.principal_kind == mapping.principal_kind
                    && rm.principal_id == mapping.principal_id
                    && rm.client_uuid == mapping.client_uuid
                    && rm.role_name == mapping.role_name)
            });
            // Also reflect on user / group representation for immediate claim evaluation
            if mapping.principal_kind == "user" {
                let client_id_opt = mapping.client_uuid.as_ref().and_then(|uuid| {
                    st.clients
                        .iter()
                        .find(|c| c.realm_id == mapping.realm_id && &c.id == uuid)
                        .map(|c| c.client_id.clone())
                });
                if let Some(u) = st
                    .users
                    .iter_mut()
                    .find(|u| u.realm_id == mapping.realm_id && u.id == mapping.principal_id)
                {
                    if let Some(cid) = client_id_opt {
                        let list = u.client_roles.entry(cid).or_default();
                        if !list.contains(&mapping.role_name) {
                            list.push(mapping.role_name.clone());
                        }
                    } else if !u.realm_roles.contains(&mapping.role_name) {
                        u.realm_roles.push(mapping.role_name.clone());
                    }
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
                .find(|c| {
                    c.realm_id == client.realm_id
                        && (c.id == client.id || c.client_id == client.client_id)
                })
                .cloned()
            {
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
                        created_timestamp: Some(Utc::now().timestamp_millis()),
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
            let sa_user = format!("service-account-{}", removed.client_id);
            // Only the client's own service account goes with it, never a person of that name.
            st.users.retain(|u| {
                !(u.realm_id == realm_id
                    && u.username.eq_ignore_ascii_case(&sa_user)
                    && u.service_account_client_id.as_deref() == Some(removed.client_id.as_str()))
            });
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
            let now = Utc::now();
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
        now: chrono::DateTime<Utc>,
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
            let now = Utc::now();
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
                        created_at: Utc::now(),
                        last_access: Utc::now(),
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
            let now = Utc::now();
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
            let now = Utc::now();
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
            let now = Utc::now();
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
            for rf in st
                .refresh_families
                .iter_mut()
                .filter(|rf| rf.realm_id == realm_id && rf.sso_session_id == session_id)
            {
                rf.revoked = true;
            }
            Ok(Some(removed))
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
            let now = Utc::now();
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
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        Box::pin(async move {
            let cap = limit.clamp(1, 50);
            let now = Utc::now();
            let mut st = self.inner.write();
            let mut claimed = Vec::new();
            for row in st.email_outbox.iter_mut() {
                if claimed.len() >= cap {
                    break;
                }
                if row.realm_id == realm_id && row.status == "pending" && row.next_attempt_at <= now
                {
                    row.status = "sending".into();
                    row.attempts = row.attempts.saturating_add(1);
                    claimed.push(row.clone());
                }
            }
            Ok(claimed)
        })
    }

    fn complete_outbox_row<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
        sent: bool,
        error: Option<String>,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        max_attempts: u32,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut st = self.inner.write();
            if let Some(row) = st
                .email_outbox
                .iter_mut()
                .find(|r| r.realm_id == realm_id && r.id == id)
            {
                if sent {
                    row.status = "sent".into();
                    row.sent_at = Some(Utc::now());
                    row.last_error = None;
                } else {
                    row.last_error = error;
                    if row.attempts >= max_attempts {
                        row.status = "failed".into();
                    } else {
                        row.status = "pending".into();
                        row.next_attempt_at =
                            next_attempt_at.unwrap_or_else(|| Utc::now() + Duration::seconds(5));
                    }
                }
            }
            Ok(())
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
            row.next_attempt_at = Utc::now();
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
            let now = Utc::now();
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

            let _ = Duration::seconds(0);
            Ok(removed)
        })
    }
}
