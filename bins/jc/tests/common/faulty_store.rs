//! A kaisatsu `Store` over another one that refuses the calls a test names (T-7637): the failure
//! arm of every store call a jc module makes is forced with it, never by a broken database.
//! Unit tests in `src/` include this file by `#[path]`; it is test code, kept out of the
//! coverage fold with the rest of `tests/`.

use kaisatsu_core::authz::{Authorizer, RequestCtx};
use kaisatsu_core::store::StoreFuture;
use kaisatsu_core::*;
use odata_table::{Answer, QueryOptions};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The error every refused call answers.
pub const REFUSED: &str = "injected store failure";

pub struct FaultyStore {
    inner: Arc<dyn Store>,
    failing: Mutex<BTreeSet<&'static str>>,
}

impl FaultyStore {
    pub fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            failing: Mutex::new(BTreeSet::new()),
        }
    }

    /// From now on the store method `method` answers [`REFUSED`].
    pub fn fail(&self, method: &'static str) {
        self.failing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(method);
    }

    fn fails(&self, method: &str) -> bool {
        self.failing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(method)
    }
}

fn refused<'a, T: Send + 'a>() -> StoreFuture<'a, T> {
    Box::pin(async { Err(KaisatsuError::Storage(REFUSED.to_owned())) })
}

impl Store for FaultyStore {
    fn query_entity_set<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        authorizer: &'a dyn Authorizer,
        set_name: &'a str,
        options: &'a QueryOptions,
        max_page: u64,
    ) -> StoreFuture<'a, Answer> {
        if self.fails("query_entity_set") {
            return refused();
        }
        self.inner
            .query_entity_set(ctx, authorizer, set_name, options, max_page)
    }
    fn get_realm<'a>(&'a self, realm_name: &'a str) -> StoreFuture<'a, Option<RealmConfig>> {
        if self.fails("get_realm") {
            return refused();
        }
        self.inner.get_realm(realm_name)
    }
    fn upsert_realm<'a>(&'a self, realm: RealmConfig) -> StoreFuture<'a, ()> {
        if self.fails("upsert_realm") {
            return refused();
        }
        self.inner.upsert_realm(realm)
    }
    fn list_realms<'a>(&'a self) -> StoreFuture<'a, Vec<RealmConfig>> {
        if self.fails("list_realms") {
            return refused();
        }
        self.inner.list_realms()
    }
    fn upsert_organization<'a>(&'a self, org: Organization) -> StoreFuture<'a, ()> {
        if self.fails("upsert_organization") {
            return refused();
        }
        self.inner.upsert_organization(org)
    }
    fn get_organization<'a>(
        &'a self,
        realm_id: &'a str,
        org_id_or_alias: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        if self.fails("get_organization") {
            return refused();
        }
        self.inner.get_organization(realm_id, org_id_or_alias)
    }
    fn list_organizations<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<Organization>> {
        if self.fails("list_organizations") {
            return refused();
        }
        self.inner.list_organizations(realm_id, allowed_orgs)
    }
    fn find_org_by_domain<'a>(
        &'a self,
        realm_id: &'a str,
        domain: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        if self.fails("find_org_by_domain") {
            return refused();
        }
        self.inner.find_org_by_domain(realm_id, domain)
    }
    fn upsert_membership<'a>(&'a self, m: Membership) -> StoreFuture<'a, ()> {
        if self.fails("upsert_membership") {
            return refused();
        }
        self.inner.upsert_membership(m)
    }
    fn list_memberships_for_person<'a>(
        &'a self,
        realm_id: &'a str,
        person_id: &'a str,
    ) -> StoreFuture<'a, Vec<Membership>> {
        if self.fails("list_memberships_for_person") {
            return refused();
        }
        self.inner.list_memberships_for_person(realm_id, person_id)
    }
    fn create_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation> {
        if self.fails("create_user") {
            return refused();
        }
        self.inner.create_user(user)
    }
    fn update_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation> {
        if self.fails("update_user") {
            return refused();
        }
        self.inner.update_user(user)
    }
    fn set_user_grants<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        realm_roles: &'a [String],
        client_roles: &'a std::collections::HashMap<String, Vec<String>>,
    ) -> StoreFuture<'a, ()> {
        if self.fails("set_user_grants") {
            return refused();
        }
        self.inner
            .set_user_grants(realm_id, user_id, realm_roles, client_roles)
    }
    fn delete_user<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_user") {
            return refused();
        }
        self.inner.delete_user(realm_id, allowed_orgs, user_id)
    }
    fn get_user_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        if self.fails("get_user_by_id") {
            return refused();
        }
        self.inner.get_user_by_id(realm_id, allowed_orgs, user_id)
    }
    fn get_user_by_username_or_email<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        identifier: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        if self.fails("get_user_by_username_or_email") {
            return refused();
        }
        self.inner
            .get_user_by_username_or_email(realm_id, allowed_orgs, identifier)
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
        if self.fails("list_users") {
            return refused();
        }
        self.inner.list_users(
            realm_id,
            allowed_orgs,
            search,
            email,
            username,
            exact,
            first,
            max,
        )
    }
    fn find_users<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        filter: &'a [FieldMatch<PersonField>],
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, ListPage<UserRepresentation>> {
        if self.fails("find_users") {
            return refused();
        }
        self.inner
            .find_users(realm_id, allowed_orgs, filter, first, max)
    }
    fn upsert_credential<'a>(&'a self, cred: CredentialRecord) -> StoreFuture<'a, ()> {
        if self.fails("upsert_credential") {
            return refused();
        }
        self.inner.upsert_credential(cred)
    }
    fn list_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<CredentialRecord>> {
        if self.fails("list_credentials") {
            return refused();
        }
        self.inner.list_credentials(realm_id, user_id)
    }
    fn delete_credential<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        cred_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_credential") {
            return refused();
        }
        self.inner.delete_credential(realm_id, user_id, cred_id)
    }
    fn upsert_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        if self.fails("upsert_group") {
            return refused();
        }
        self.inner.upsert_group(group)
    }
    fn update_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        if self.fails("update_group") {
            return refused();
        }
        self.inner.update_group(group)
    }
    fn get_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        if self.fails("get_group") {
            return refused();
        }
        self.inner.get_group(realm_id, allowed_orgs, group_id)
    }
    fn get_group_by_path<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        path: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        if self.fails("get_group_by_path") {
            return refused();
        }
        self.inner.get_group_by_path(realm_id, allowed_orgs, path)
    }
    fn list_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        exact: bool,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        if self.fails("list_groups") {
            return refused();
        }
        self.inner
            .list_groups(realm_id, allowed_orgs, search, exact)
    }
    fn list_child_groups<'a>(
        &'a self,
        allowed_orgs: Option<&'a [String]>,
        parent: &'a GroupRepresentation,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        if self.fails("list_child_groups") {
            return refused();
        }
        self.inner.list_child_groups(allowed_orgs, parent)
    }
    fn find_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        filter: &'a [FieldMatch<GroupField>],
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, ListPage<GroupWithMembers>> {
        if self.fails("find_groups") {
            return refused();
        }
        self.inner
            .find_groups(realm_id, allowed_orgs, filter, first, max)
    }
    fn delete_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_group") {
            return refused();
        }
        self.inner.delete_group(realm_id, allowed_orgs, group_id)
    }
    fn add_user_to_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("add_user_to_group") {
            return refused();
        }
        self.inner.add_user_to_group(realm_id, user_id, group_id)
    }
    fn remove_user_from_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("remove_user_from_group") {
            return refused();
        }
        self.inner
            .remove_user_from_group(realm_id, user_id, group_id)
    }
    fn list_user_groups<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        if self.fails("list_user_groups") {
            return refused();
        }
        self.inner.list_user_groups(realm_id, user_id)
    }
    fn list_group_members<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        if self.fails("list_group_members") {
            return refused();
        }
        self.inner
            .list_group_members(realm_id, allowed_orgs, group_id, first, max)
    }
    fn upsert_role<'a>(&'a self, role: RoleRepresentation) -> StoreFuture<'a, RoleRepresentation> {
        if self.fails("upsert_role") {
            return refused();
        }
        self.inner.upsert_role(role)
    }
    fn list_roles<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleRepresentation>> {
        if self.fails("list_roles") {
            return refused();
        }
        self.inner.list_roles(realm_id, container_id)
    }
    fn delete_role<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_role") {
            return refused();
        }
        self.inner.delete_role(realm_id, container_id, role_name)
    }
    fn add_role_mapping<'a>(&'a self, mapping: RoleMappingRecord) -> StoreFuture<'a, ()> {
        if self.fails("add_role_mapping") {
            return refused();
        }
        self.inner.add_role_mapping(mapping)
    }
    fn remove_role_mapping<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("remove_role_mapping") {
            return refused();
        }
        self.inner.remove_role_mapping(
            realm_id,
            principal_kind,
            principal_id,
            client_uuid,
            role_name,
        )
    }
    fn list_role_mappings_for_principal<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleMappingRecord>> {
        if self.fails("list_role_mappings_for_principal") {
            return refused();
        }
        self.inner.list_role_mappings_for_principal(
            realm_id,
            principal_kind,
            principal_id,
            client_uuid,
        )
    }
    fn list_principals_with_client_role<'a>(
        &'a self,
        realm_id: &'a str,
        client_uuid: &'a str,
        role_name: &'a str,
        principal_kind: &'a str,
    ) -> StoreFuture<'a, Vec<String>> {
        if self.fails("list_principals_with_client_role") {
            return refused();
        }
        self.inner.list_principals_with_client_role(
            realm_id,
            client_uuid,
            role_name,
            principal_kind,
        )
    }
    fn upsert_client<'a>(
        &'a self,
        client: ClientRepresentation,
    ) -> StoreFuture<'a, ClientRepresentation> {
        if self.fails("upsert_client") {
            return refused();
        }
        self.inner.upsert_client(client)
    }
    fn get_client_by_uuid<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        if self.fails("get_client_by_uuid") {
            return refused();
        }
        self.inner.get_client_by_uuid(realm_id, allowed_orgs, uuid)
    }
    fn get_client_by_client_id<'a>(
        &'a self,
        realm_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        if self.fails("get_client_by_client_id") {
            return refused();
        }
        self.inner.get_client_by_client_id(realm_id, client_id)
    }
    fn list_clients<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        client_id_filter: Option<&'a str>,
        search: bool,
    ) -> StoreFuture<'a, Vec<ClientRepresentation>> {
        if self.fails("list_clients") {
            return refused();
        }
        self.inner
            .list_clients(realm_id, allowed_orgs, client_id_filter, search)
    }
    fn delete_client<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_client") {
            return refused();
        }
        self.inner.delete_client(realm_id, allowed_orgs, uuid)
    }
    fn upsert_client_scope<'a>(
        &'a self,
        scope: ClientScopeRepresentation,
    ) -> StoreFuture<'a, ClientScopeRepresentation> {
        if self.fails("upsert_client_scope") {
            return refused();
        }
        self.inner.upsert_client_scope(scope)
    }
    fn get_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientScopeRepresentation>> {
        if self.fails("get_client_scope") {
            return refused();
        }
        self.inner.get_client_scope(realm_id, scope_id)
    }
    fn list_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        if self.fails("list_client_scopes") {
            return refused();
        }
        self.inner.list_client_scopes(realm_id)
    }
    fn get_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        if self.fails("get_client_scopes") {
            return refused();
        }
        self.inner.get_client_scopes(realm_id, keys)
    }
    fn list_client_scopes_with_prefix<'a>(
        &'a self,
        realm_id: &'a str,
        prefix: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        if self.fails("list_client_scopes_with_prefix") {
            return refused();
        }
        self.inner.list_client_scopes_with_prefix(realm_id, prefix)
    }
    fn delete_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_client_scope") {
            return refused();
        }
        self.inner.delete_client_scope(realm_id, scope_id)
    }
    fn upsert_idp<'a>(&'a self, idp: IdentityProviderRepresentation) -> StoreFuture<'a, ()> {
        if self.fails("upsert_idp") {
            return refused();
        }
        self.inner.upsert_idp(idp)
    }
    fn get_idp<'a>(
        &'a self,
        realm_id: &'a str,
        alias: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderRepresentation>> {
        if self.fails("get_idp") {
            return refused();
        }
        self.inner.get_idp(realm_id, alias)
    }
    fn list_idps<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderRepresentation>> {
        if self.fails("list_idps") {
            return refused();
        }
        self.inner.list_idps(realm_id)
    }
    fn upsert_signing_key<'a>(&'a self, key: SigningKeyRecord) -> StoreFuture<'a, ()> {
        if self.fails("upsert_signing_key") {
            return refused();
        }
        self.inner.upsert_signing_key(key)
    }
    fn list_signing_keys<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<SigningKeyRecord>> {
        if self.fails("list_signing_keys") {
            return refused();
        }
        self.inner.list_signing_keys(realm_id)
    }
    fn ensure_signing_key<'a>(
        &'a self,
        realm_id: &'a str,
        algorithm: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("ensure_signing_key") {
            return refused();
        }
        self.inner.ensure_signing_key(realm_id, algorithm)
    }
    fn put_auth_session<'a>(&'a self, session: AuthSession) -> StoreFuture<'a, ()> {
        if self.fails("put_auth_session") {
            return refused();
        }
        self.inner.put_auth_session(session)
    }
    fn get_auth_session_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
    ) -> StoreFuture<'a, Option<AuthSession>> {
        if self.fails("get_auth_session_by_id") {
            return refused();
        }
        self.inner.get_auth_session_by_id(realm_id, id)
    }
    fn delete_auth_session<'a>(&'a self, realm_id: &'a str, id: &'a str) -> StoreFuture<'a, ()> {
        if self.fails("delete_auth_session") {
            return refused();
        }
        self.inner.delete_auth_session(realm_id, id)
    }
    fn put_auth_code<'a>(&'a self, code: AuthorizationCodeRecord) -> StoreFuture<'a, ()> {
        if self.fails("put_auth_code") {
            return refused();
        }
        self.inner.put_auth_code(code)
    }
    fn consume_auth_code<'a>(
        &'a self,
        realm_id: &'a str,
        code_hash: &'a str,
    ) -> StoreFuture<'a, (Option<AuthorizationCodeRecord>, bool)> {
        if self.fails("consume_auth_code") {
            return refused();
        }
        self.inner.consume_auth_code(realm_id, code_hash)
    }
    fn put_par_request<'a>(&'a self, par: ParRequestRecord) -> StoreFuture<'a, ()> {
        if self.fails("put_par_request") {
            return refused();
        }
        self.inner.put_par_request(par)
    }
    fn consume_par_request<'a>(
        &'a self,
        realm_id: &'a str,
        request_uri_hash: &'a str,
    ) -> StoreFuture<'a, Option<ParRequestRecord>> {
        if self.fails("consume_par_request") {
            return refused();
        }
        self.inner.consume_par_request(realm_id, request_uri_hash)
    }
    fn put_device_code<'a>(&'a self, rec: DeviceCodeRecord) -> StoreFuture<'a, ()> {
        if self.fails("put_device_code") {
            return refused();
        }
        self.inner.put_device_code(rec)
    }
    fn get_device_code_by_user_code<'a>(
        &'a self,
        realm_id: &'a str,
        user_code_hash: &'a str,
    ) -> StoreFuture<'a, Option<DeviceCodeRecord>> {
        if self.fails("get_device_code_by_user_code") {
            return refused();
        }
        self.inner
            .get_device_code_by_user_code(realm_id, user_code_hash)
    }
    fn decide_device_code<'a>(&'a self, rec: DeviceCodeRecord) -> StoreFuture<'a, bool> {
        if self.fails("decide_device_code") {
            return refused();
        }
        self.inner.decide_device_code(rec)
    }
    fn poll_device_code<'a>(
        &'a self,
        realm_id: &'a str,
        device_code_hash: &'a str,
        client_id: &'a str,
        now: jc_time::TimestampTz,
    ) -> StoreFuture<'a, Option<(DeviceCodeRecord, DevicePoll)>> {
        if self.fails("poll_device_code") {
            return refused();
        }
        self.inner
            .poll_device_code(realm_id, device_code_hash, client_id, now)
    }
    fn upsert_sso_session<'a>(&'a self, session: SsoSession) -> StoreFuture<'a, ()> {
        if self.fails("upsert_sso_session") {
            return refused();
        }
        self.inner.upsert_sso_session(session)
    }
    fn get_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        if self.fails("get_sso_session") {
            return refused();
        }
        self.inner.get_sso_session(realm_id, session_id)
    }
    fn get_sso_session_by_handle<'a>(
        &'a self,
        realm_id: &'a str,
        handle_hash: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        if self.fails("get_sso_session_by_handle") {
            return refused();
        }
        self.inner.get_sso_session_by_handle(realm_id, handle_hash)
    }
    fn list_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        if self.fails("list_user_sso_sessions") {
            return refused();
        }
        self.inner.list_user_sso_sessions(realm_id, user_id)
    }
    fn delete_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        if self.fails("delete_sso_session") {
            return refused();
        }
        self.inner.delete_sso_session(realm_id, session_id)
    }
    fn delete_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        if self.fails("delete_user_sso_sessions") {
            return refused();
        }
        self.inner.delete_user_sso_sessions(realm_id, user_id)
    }
    fn upsert_offline_session<'a>(&'a self, session: OfflineSession) -> StoreFuture<'a, ()> {
        if self.fails("upsert_offline_session") {
            return refused();
        }
        self.inner.upsert_offline_session(session)
    }
    fn delete_user_client_offline_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("delete_user_client_offline_sessions") {
            return refused();
        }
        self.inner
            .delete_user_client_offline_sessions(realm_id, user_id, client_id)
    }
    fn upsert_refresh_family<'a>(&'a self, family: RefreshTokenFamily) -> StoreFuture<'a, ()> {
        if self.fails("upsert_refresh_family") {
            return refused();
        }
        self.inner.upsert_refresh_family(family)
    }
    fn get_refresh_family<'a>(
        &'a self,
        realm_id: &'a str,
        family_id: &'a str,
    ) -> StoreFuture<'a, Option<RefreshTokenFamily>> {
        if self.fails("get_refresh_family") {
            return refused();
        }
        self.inner.get_refresh_family(realm_id, family_id)
    }
    fn rotate_refresh_family<'a>(
        &'a self,
        family: RefreshTokenFamily,
        expected_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("rotate_refresh_family") {
            return refused();
        }
        self.inner.rotate_refresh_family(family, expected_hash)
    }
    fn revoke_offline_families_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("revoke_offline_families_for_user") {
            return refused();
        }
        self.inner
            .revoke_offline_families_for_user(realm_id, user_id)
    }
    fn revoke_refresh_families_for_session<'a>(
        &'a self,
        realm_id: &'a str,
        sso_session_id: &'a str,
        client_id: Option<&'a str>,
    ) -> StoreFuture<'a, ()> {
        if self.fails("revoke_refresh_families_for_session") {
            return refused();
        }
        self.inner
            .revoke_refresh_families_for_session(realm_id, sso_session_id, client_id)
    }
    fn mark_token_revoked<'a>(&'a self, rec: RevokedTokenRecord) -> StoreFuture<'a, ()> {
        if self.fails("mark_token_revoked") {
            return refused();
        }
        self.inner.mark_token_revoked(rec)
    }
    fn is_token_revoked<'a>(
        &'a self,
        realm_id: &'a str,
        jti_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("is_token_revoked") {
            return refused();
        }
        self.inner.is_token_revoked(realm_id, jti_hash)
    }
    fn get_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, Option<LoginFailureRecord>> {
        if self.fails("get_login_failure") {
            return refused();
        }
        self.inner.get_login_failure(realm_id, target_key)
    }
    fn upsert_login_failure<'a>(&'a self, rec: LoginFailureRecord) -> StoreFuture<'a, ()> {
        if self.fails("upsert_login_failure") {
            return refused();
        }
        self.inner.upsert_login_failure(rec)
    }
    fn update_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
        update: &'a LoginFailureUpdate<'a>,
    ) -> StoreFuture<'a, LoginFailureRecord> {
        if self.fails("update_login_failure") {
            return refused();
        }
        self.inner
            .update_login_failure(realm_id, target_key, update)
    }
    fn clear_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, ()> {
        if self.fails("clear_login_failure") {
            return refused();
        }
        self.inner.clear_login_failure(realm_id, target_key)
    }
    fn upsert_consent<'a>(&'a self, consent: ConsentRecord) -> StoreFuture<'a, ()> {
        if self.fails("upsert_consent") {
            return refused();
        }
        self.inner.upsert_consent(consent)
    }
    fn list_consents<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<ConsentRecord>> {
        if self.fails("list_consents") {
            return refused();
        }
        self.inner.list_consents(realm_id, user_id)
    }
    fn delete_consent<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_consent") {
            return refused();
        }
        self.inner.delete_consent(realm_id, user_id, client_id)
    }
    fn upsert_required_action<'a>(&'a self, ra: RequiredActionConfig) -> StoreFuture<'a, ()> {
        if self.fails("upsert_required_action") {
            return refused();
        }
        self.inner.upsert_required_action(ra)
    }
    fn list_required_actions<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<RequiredActionConfig>> {
        if self.fails("list_required_actions") {
            return refused();
        }
        self.inner.list_required_actions(realm_id)
    }
    fn upsert_branding<'a>(&'a self, branding: BrandingBlock) -> StoreFuture<'a, ()> {
        if self.fails("upsert_branding") {
            return refused();
        }
        self.inner.upsert_branding(branding)
    }
    fn get_branding<'a>(
        &'a self,
        realm_id: &'a str,
        org_id: Option<&'a str>,
    ) -> StoreFuture<'a, Option<BrandingBlock>> {
        if self.fails("get_branding") {
            return refused();
        }
        self.inner.get_branding(realm_id, org_id)
    }
    fn append_event<'a>(&'a self, event: AuditEvent) -> StoreFuture<'a, ()> {
        if self.fails("append_event") {
            return refused();
        }
        self.inner.append_event(event)
    }
    fn list_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AuditEvent>> {
        if self.fails("list_events") {
            return refused();
        }
        self.inner.list_events(realm_id, allowed_orgs)
    }
    fn append_admin_event<'a>(&'a self, event: AdminEvent) -> StoreFuture<'a, ()> {
        if self.fails("append_admin_event") {
            return refused();
        }
        self.inner.append_admin_event(event)
    }
    fn list_admin_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AdminEvent>> {
        if self.fails("list_admin_events") {
            return refused();
        }
        self.inner.list_admin_events(realm_id, allowed_orgs)
    }
    fn insert_webauthn_credential<'a>(
        &'a self,
        cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, WebAuthnCredentialRecord> {
        if self.fails("insert_webauthn_credential") {
            return refused();
        }
        self.inner.insert_webauthn_credential(cred)
    }
    fn update_webauthn_credential<'a>(
        &'a self,
        cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, ()> {
        if self.fails("update_webauthn_credential") {
            return refused();
        }
        self.inner.update_webauthn_credential(cred)
    }
    fn get_webauthn_by_credential_id<'a>(
        &'a self,
        realm_id: &'a str,
        credential_id: &'a str,
    ) -> StoreFuture<'a, Option<WebAuthnCredentialRecord>> {
        if self.fails("get_webauthn_by_credential_id") {
            return refused();
        }
        self.inner
            .get_webauthn_by_credential_id(realm_id, credential_id)
    }
    fn list_webauthn_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<WebAuthnCredentialRecord>> {
        if self.fails("list_webauthn_credentials") {
            return refused();
        }
        self.inner.list_webauthn_credentials(realm_id, user_id)
    }
    fn delete_webauthn_credential<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, bool> {
        if self.fails("delete_webauthn_credential") {
            return refused();
        }
        self.inner
            .delete_webauthn_credential(realm_id, allowed_orgs, id)
    }
    fn update_user_with_outbox<'a>(
        &'a self,
        user: UserRepresentation,
        outbox: EmailOutboxRecord,
    ) -> StoreFuture<'a, UserRepresentation> {
        if self.fails("update_user_with_outbox") {
            return refused();
        }
        self.inner.update_user_with_outbox(user, outbox)
    }
    fn enqueue_outbox<'a>(&'a self, outbox: EmailOutboxRecord) -> StoreFuture<'a, ()> {
        if self.fails("enqueue_outbox") {
            return refused();
        }
        self.inner.enqueue_outbox(outbox)
    }
    fn claim_outbox_batch<'a>(
        &'a self,
        realm_id: &'a str,
        limit: usize,
        lease: std::time::Duration,
        max_attempts: u32,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        if self.fails("claim_outbox_batch") {
            return refused();
        }
        self.inner
            .claim_outbox_batch(realm_id, limit, lease, max_attempts)
    }
    fn complete_outbox_row<'a>(
        &'a self,
        claimed: &'a EmailOutboxRecord,
        sent: bool,
        error: Option<String>,
        next_attempt_at: Option<jc_time::TimestampTz>,
        max_attempts: u32,
    ) -> StoreFuture<'a, bool> {
        if self.fails("complete_outbox_row") {
            return refused();
        }
        self.inner
            .complete_outbox_row(claimed, sent, error, next_attempt_at, max_attempts)
    }
    fn retry_outbox_row<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, Option<EmailOutboxRecord>> {
        if self.fails("retry_outbox_row") {
            return refused();
        }
        self.inner.retry_outbox_row(realm_id, allowed_orgs, id)
    }
    fn list_outbox_rows<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        if self.fails("list_outbox_rows") {
            return refused();
        }
        self.inner.list_outbox_rows(realm_id, allowed_orgs)
    }
    fn upsert_idp_link<'a>(&'a self, link: IdentityProviderLinkRecord) -> StoreFuture<'a, ()> {
        if self.fails("upsert_idp_link") {
            return refused();
        }
        self.inner.upsert_idp_link(link)
    }
    fn get_idp_link_by_external<'a>(
        &'a self,
        realm_id: &'a str,
        idp_alias: &'a str,
        external_user_id: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderLinkRecord>> {
        if self.fails("get_idp_link_by_external") {
            return refused();
        }
        self.inner
            .get_idp_link_by_external(realm_id, idp_alias, external_user_id)
    }
    fn list_idp_links_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderLinkRecord>> {
        if self.fails("list_idp_links_for_user") {
            return refused();
        }
        self.inner.list_idp_links_for_user(realm_id, user_id)
    }
    fn sweep_expired<'a>(&'a self) -> StoreFuture<'a, u64> {
        if self.fails("sweep_expired") {
            return refused();
        }
        self.inner.sweep_expired()
    }
}
