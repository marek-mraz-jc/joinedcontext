//! A [`Store`] that forwards every call to an inner store and fails the methods a test names,
//! so a route's store-error arms run (T-7802). `fail(method, skip, times, make)` lets `skip` calls
//! through, then answers the next `times` calls with `make()`; `empty(method, skip, times)` answers
//! them with nothing found instead (`None`, an empty list, `false`, `()` without the write), as a
//! row another request deleted or a lost race reads.

use kaisatsu_core::store::StoreFuture;
use kaisatsu_core::*;
use odata_table::{Answer, QueryOptions};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

enum Outcome {
    Fail(KaisatsuError),
    Empty,
}

struct Fault {
    skip: u32,
    times: u32,
    make: Option<fn() -> KaisatsuError>,
}

pub struct FaultStore {
    inner: Arc<dyn Store>,
    faults: Mutex<HashMap<&'static str, Fault>>,
}

/// The error a failing database answers with.
pub fn storage() -> KaisatsuError {
    KaisatsuError::Storage("injected store failure".into())
}

impl FaultStore {
    pub fn new(inner: Arc<dyn Store>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            faults: Mutex::new(HashMap::new()),
        })
    }

    pub fn fail(&self, method: &'static str, skip: u32, times: u32, make: fn() -> KaisatsuError) {
        let fault = Fault {
            skip,
            times,
            make: Some(make),
        };
        self.faults.lock().expect("faults").insert(method, fault);
    }

    /// Fails every later call of `method` with a storage error.
    pub fn fail_always(&self, method: &'static str) {
        self.fail(method, 0, u32::MAX, storage);
    }

    /// Answers the calls after the first `skip` with nothing found, `times` times.
    pub fn empty(&self, method: &'static str, skip: u32, times: u32) {
        let fault = Fault {
            skip,
            times,
            make: None,
        };
        self.faults.lock().expect("faults").insert(method, fault);
    }

    pub fn clear(&self) {
        self.faults.lock().expect("faults").clear();
    }

    fn outcome(&self, method: &str) -> Option<Outcome> {
        let mut faults = self.faults.lock().expect("faults");
        let f = faults.get_mut(method)?;
        if f.skip > 0 {
            f.skip -= 1;
            return None;
        }
        if f.times == 0 {
            return None;
        }
        f.times -= 1;
        Some(f.make.map_or(Outcome::Empty, |make| Outcome::Fail(make())))
    }
}

impl Store for FaultStore {
    fn query_entity_set<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        authorizer: &'a dyn Authorizer,
        set_name: &'a str,
        options: &'a QueryOptions,
        max_page: u64,
    ) -> StoreFuture<'a, Answer> {
        match self.outcome("query_entity_set") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self
                .inner
                .query_entity_set(ctx, authorizer, set_name, options, max_page),
        }
    }

    fn get_realm<'a>(&'a self, realm_name: &'a str) -> StoreFuture<'a, Option<RealmConfig>> {
        match self.outcome("get_realm") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_realm(realm_name),
        }
    }

    fn upsert_realm<'a>(&'a self, realm: RealmConfig) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_realm") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_realm(realm),
        }
    }

    fn list_realms<'a>(&'a self) -> StoreFuture<'a, Vec<RealmConfig>> {
        match self.outcome("list_realms") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_realms(),
        }
    }

    fn upsert_organization<'a>(&'a self, org: Organization) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_organization") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_organization(org),
        }
    }

    fn get_organization<'a>(
        &'a self,
        realm_id: &'a str,
        org_id_or_alias: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        match self.outcome("get_organization") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_organization(realm_id, org_id_or_alias),
        }
    }

    fn list_organizations<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<Organization>> {
        match self.outcome("list_organizations") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_organizations(realm_id, allowed_orgs),
        }
    }

    fn find_org_by_domain<'a>(
        &'a self,
        realm_id: &'a str,
        domain: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        match self.outcome("find_org_by_domain") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.find_org_by_domain(realm_id, domain),
        }
    }

    fn upsert_membership<'a>(&'a self, m: Membership) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_membership") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_membership(m),
        }
    }

    fn list_memberships_for_person<'a>(
        &'a self,
        realm_id: &'a str,
        person_id: &'a str,
    ) -> StoreFuture<'a, Vec<Membership>> {
        match self.outcome("list_memberships_for_person") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_memberships_for_person(realm_id, person_id),
        }
    }

    fn create_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation> {
        match self.outcome("create_user") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.create_user(user),
        }
    }

    fn update_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation> {
        match self.outcome("update_user") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.update_user(user),
        }
    }

    fn set_user_grants<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        realm_roles: &'a [String],
        client_roles: &'a std::collections::HashMap<String, Vec<String>>,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("set_user_grants") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .set_user_grants(realm_id, user_id, realm_roles, client_roles),
        }
    }

    fn delete_user<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_user") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_user(realm_id, allowed_orgs, user_id),
        }
    }

    fn get_user_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        match self.outcome("get_user_by_id") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_user_by_id(realm_id, allowed_orgs, user_id),
        }
    }

    fn get_user_by_username_or_email<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        identifier: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        match self.outcome("get_user_by_username_or_email") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .get_user_by_username_or_email(realm_id, allowed_orgs, identifier),
        }
    }

    #[allow(clippy::too_many_arguments)]
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
        match self.outcome("list_users") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_users(
                realm_id,
                allowed_orgs,
                search,
                email,
                username,
                exact,
                first,
                max,
            ),
        }
    }

    fn find_users<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        filter: &'a [FieldMatch<PersonField>],
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, ListPage<UserRepresentation>> {
        match self.outcome("find_users") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self
                .inner
                .find_users(realm_id, allowed_orgs, filter, first, max),
        }
    }

    fn upsert_credential<'a>(&'a self, cred: CredentialRecord) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_credential") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_credential(cred),
        }
    }

    fn list_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<CredentialRecord>> {
        match self.outcome("list_credentials") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_credentials(realm_id, user_id),
        }
    }

    fn delete_credential<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        cred_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_credential") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_credential(realm_id, user_id, cred_id),
        }
    }

    fn upsert_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        match self.outcome("upsert_group") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.upsert_group(group),
        }
    }

    fn update_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        match self.outcome("update_group") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.update_group(group),
        }
    }

    fn get_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        match self.outcome("get_group") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_group(realm_id, allowed_orgs, group_id),
        }
    }

    fn get_group_by_path<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        path: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        match self.outcome("get_group_by_path") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_group_by_path(realm_id, allowed_orgs, path),
        }
    }

    fn list_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        exact: bool,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        match self.outcome("list_groups") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .list_groups(realm_id, allowed_orgs, search, exact),
        }
    }

    fn list_child_groups<'a>(
        &'a self,
        allowed_orgs: Option<&'a [String]>,
        parent: &'a GroupRepresentation,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        match self.outcome("list_child_groups") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_child_groups(allowed_orgs, parent),
        }
    }

    fn find_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        filter: &'a [FieldMatch<GroupField>],
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, ListPage<GroupWithMembers>> {
        match self.outcome("find_groups") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self
                .inner
                .find_groups(realm_id, allowed_orgs, filter, first, max),
        }
    }

    fn delete_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_group") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_group(realm_id, allowed_orgs, group_id),
        }
    }

    fn add_user_to_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("add_user_to_group") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.add_user_to_group(realm_id, user_id, group_id),
        }
    }

    fn remove_user_from_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("remove_user_from_group") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .remove_user_from_group(realm_id, user_id, group_id),
        }
    }

    fn list_user_groups<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        match self.outcome("list_user_groups") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_user_groups(realm_id, user_id),
        }
    }

    fn list_group_members<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        match self.outcome("list_group_members") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .list_group_members(realm_id, allowed_orgs, group_id, first, max),
        }
    }

    fn upsert_role<'a>(&'a self, role: RoleRepresentation) -> StoreFuture<'a, RoleRepresentation> {
        match self.outcome("upsert_role") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.upsert_role(role),
        }
    }

    fn list_roles<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleRepresentation>> {
        match self.outcome("list_roles") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_roles(realm_id, container_id),
        }
    }

    fn delete_role<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_role") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_role(realm_id, container_id, role_name),
        }
    }

    fn add_role_mapping<'a>(&'a self, mapping: RoleMappingRecord) -> StoreFuture<'a, ()> {
        match self.outcome("add_role_mapping") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.add_role_mapping(mapping),
        }
    }

    fn remove_role_mapping<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("remove_role_mapping") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.remove_role_mapping(
                realm_id,
                principal_kind,
                principal_id,
                client_uuid,
                role_name,
            ),
        }
    }

    fn list_role_mappings_for_principal<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleMappingRecord>> {
        match self.outcome("list_role_mappings_for_principal") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_role_mappings_for_principal(
                realm_id,
                principal_kind,
                principal_id,
                client_uuid,
            ),
        }
    }

    fn list_principals_with_client_role<'a>(
        &'a self,
        realm_id: &'a str,
        client_uuid: &'a str,
        role_name: &'a str,
        principal_kind: &'a str,
    ) -> StoreFuture<'a, Vec<String>> {
        match self.outcome("list_principals_with_client_role") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_principals_with_client_role(
                realm_id,
                client_uuid,
                role_name,
                principal_kind,
            ),
        }
    }

    fn upsert_client<'a>(
        &'a self,
        client: ClientRepresentation,
    ) -> StoreFuture<'a, ClientRepresentation> {
        match self.outcome("upsert_client") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.upsert_client(client),
        }
    }

    fn get_client_by_uuid<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        match self.outcome("get_client_by_uuid") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_client_by_uuid(realm_id, allowed_orgs, uuid),
        }
    }

    fn get_client_by_client_id<'a>(
        &'a self,
        realm_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        match self.outcome("get_client_by_client_id") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_client_by_client_id(realm_id, client_id),
        }
    }

    fn list_clients<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        client_id_filter: Option<&'a str>,
        search: bool,
    ) -> StoreFuture<'a, Vec<ClientRepresentation>> {
        match self.outcome("list_clients") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .list_clients(realm_id, allowed_orgs, client_id_filter, search),
        }
    }

    fn delete_client<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_client") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_client(realm_id, allowed_orgs, uuid),
        }
    }

    fn upsert_client_scope<'a>(
        &'a self,
        scope: ClientScopeRepresentation,
    ) -> StoreFuture<'a, ClientScopeRepresentation> {
        match self.outcome("upsert_client_scope") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.upsert_client_scope(scope),
        }
    }

    fn get_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientScopeRepresentation>> {
        match self.outcome("get_client_scope") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_client_scope(realm_id, scope_id),
        }
    }

    fn list_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        match self.outcome("list_client_scopes") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_client_scopes(realm_id),
        }
    }

    fn get_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        match self.outcome("get_client_scopes") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_client_scopes(realm_id, keys),
        }
    }

    fn list_client_scopes_with_prefix<'a>(
        &'a self,
        realm_id: &'a str,
        prefix: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        match self.outcome("list_client_scopes_with_prefix") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_client_scopes_with_prefix(realm_id, prefix),
        }
    }

    fn delete_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_client_scope") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_client_scope(realm_id, scope_id),
        }
    }

    fn upsert_idp<'a>(&'a self, idp: IdentityProviderRepresentation) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_idp") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_idp(idp),
        }
    }

    fn get_idp<'a>(
        &'a self,
        realm_id: &'a str,
        alias: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderRepresentation>> {
        match self.outcome("get_idp") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_idp(realm_id, alias),
        }
    }

    fn list_idps<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderRepresentation>> {
        match self.outcome("list_idps") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_idps(realm_id),
        }
    }

    fn upsert_signing_key<'a>(&'a self, key: SigningKeyRecord) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_signing_key") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_signing_key(key),
        }
    }

    fn list_signing_keys<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<SigningKeyRecord>> {
        match self.outcome("list_signing_keys") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_signing_keys(realm_id),
        }
    }

    fn ensure_signing_key<'a>(
        &'a self,
        realm_id: &'a str,
        algorithm: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("ensure_signing_key") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.ensure_signing_key(realm_id, algorithm),
        }
    }

    fn put_auth_session<'a>(&'a self, session: AuthSession) -> StoreFuture<'a, ()> {
        match self.outcome("put_auth_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.put_auth_session(session),
        }
    }

    fn get_auth_session_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
    ) -> StoreFuture<'a, Option<AuthSession>> {
        match self.outcome("get_auth_session_by_id") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_auth_session_by_id(realm_id, id),
        }
    }

    fn delete_auth_session<'a>(&'a self, realm_id: &'a str, id: &'a str) -> StoreFuture<'a, ()> {
        match self.outcome("delete_auth_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_auth_session(realm_id, id),
        }
    }

    fn put_auth_code<'a>(&'a self, code: AuthorizationCodeRecord) -> StoreFuture<'a, ()> {
        match self.outcome("put_auth_code") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.put_auth_code(code),
        }
    }

    fn consume_auth_code<'a>(
        &'a self,
        realm_id: &'a str,
        code_hash: &'a str,
    ) -> StoreFuture<'a, (Option<AuthorizationCodeRecord>, bool)> {
        match self.outcome("consume_auth_code") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.consume_auth_code(realm_id, code_hash),
        }
    }

    fn put_par_request<'a>(&'a self, par: ParRequestRecord) -> StoreFuture<'a, ()> {
        match self.outcome("put_par_request") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.put_par_request(par),
        }
    }

    fn consume_par_request<'a>(
        &'a self,
        realm_id: &'a str,
        request_uri_hash: &'a str,
    ) -> StoreFuture<'a, Option<ParRequestRecord>> {
        match self.outcome("consume_par_request") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.consume_par_request(realm_id, request_uri_hash),
        }
    }

    fn put_device_code<'a>(&'a self, rec: DeviceCodeRecord) -> StoreFuture<'a, ()> {
        match self.outcome("put_device_code") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.put_device_code(rec),
        }
    }

    fn get_device_code_by_user_code<'a>(
        &'a self,
        realm_id: &'a str,
        user_code_hash: &'a str,
    ) -> StoreFuture<'a, Option<DeviceCodeRecord>> {
        match self.outcome("get_device_code_by_user_code") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .get_device_code_by_user_code(realm_id, user_code_hash),
        }
    }

    fn decide_device_code<'a>(&'a self, rec: DeviceCodeRecord) -> StoreFuture<'a, bool> {
        match self.outcome("decide_device_code") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.decide_device_code(rec),
        }
    }

    fn poll_device_code<'a>(
        &'a self,
        realm_id: &'a str,
        device_code_hash: &'a str,
        client_id: &'a str,
        now: jc_time::TimestampTz,
    ) -> StoreFuture<'a, Option<(DeviceCodeRecord, DevicePoll)>> {
        match self.outcome("poll_device_code") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .poll_device_code(realm_id, device_code_hash, client_id, now),
        }
    }

    fn upsert_sso_session<'a>(&'a self, session: SsoSession) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_sso_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_sso_session(session),
        }
    }

    fn get_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        match self.outcome("get_sso_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_sso_session(realm_id, session_id),
        }
    }

    fn get_sso_session_by_handle<'a>(
        &'a self,
        realm_id: &'a str,
        handle_hash: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        match self.outcome("get_sso_session_by_handle") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_sso_session_by_handle(realm_id, handle_hash),
        }
    }

    fn list_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        match self.outcome("list_user_sso_sessions") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_user_sso_sessions(realm_id, user_id),
        }
    }

    fn delete_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        match self.outcome("delete_sso_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_sso_session(realm_id, session_id),
        }
    }

    fn delete_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        match self.outcome("delete_user_sso_sessions") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_user_sso_sessions(realm_id, user_id),
        }
    }

    fn upsert_offline_session<'a>(&'a self, session: OfflineSession) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_offline_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_offline_session(session),
        }
    }

    fn delete_user_client_offline_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("delete_user_client_offline_sessions") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .delete_user_client_offline_sessions(realm_id, user_id, client_id),
        }
    }

    fn upsert_refresh_family<'a>(&'a self, family: RefreshTokenFamily) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_refresh_family") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_refresh_family(family),
        }
    }

    fn get_refresh_family<'a>(
        &'a self,
        realm_id: &'a str,
        family_id: &'a str,
    ) -> StoreFuture<'a, Option<RefreshTokenFamily>> {
        match self.outcome("get_refresh_family") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_refresh_family(realm_id, family_id),
        }
    }

    fn rotate_refresh_family<'a>(
        &'a self,
        family: RefreshTokenFamily,
        expected_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("rotate_refresh_family") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.rotate_refresh_family(family, expected_hash),
        }
    }

    fn revoke_offline_families_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("revoke_offline_families_for_user") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .revoke_offline_families_for_user(realm_id, user_id),
        }
    }

    fn revoke_refresh_families_for_session<'a>(
        &'a self,
        realm_id: &'a str,
        sso_session_id: &'a str,
        client_id: Option<&'a str>,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("revoke_refresh_families_for_session") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => {
                self.inner
                    .revoke_refresh_families_for_session(realm_id, sso_session_id, client_id)
            }
        }
    }

    fn mark_token_revoked<'a>(&'a self, rec: RevokedTokenRecord) -> StoreFuture<'a, ()> {
        match self.outcome("mark_token_revoked") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.mark_token_revoked(rec),
        }
    }

    fn is_token_revoked<'a>(
        &'a self,
        realm_id: &'a str,
        jti_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("is_token_revoked") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.is_token_revoked(realm_id, jti_hash),
        }
    }

    fn get_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, Option<LoginFailureRecord>> {
        match self.outcome("get_login_failure") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_login_failure(realm_id, target_key),
        }
    }

    fn upsert_login_failure<'a>(&'a self, rec: LoginFailureRecord) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_login_failure") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_login_failure(rec),
        }
    }

    fn update_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
        update: &'a LoginFailureUpdate<'a>,
    ) -> StoreFuture<'a, LoginFailureRecord> {
        match self.outcome("update_login_failure") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self
                .inner
                .update_login_failure(realm_id, target_key, update),
        }
    }

    fn clear_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("clear_login_failure") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.clear_login_failure(realm_id, target_key),
        }
    }

    fn upsert_consent<'a>(&'a self, consent: ConsentRecord) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_consent") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_consent(consent),
        }
    }

    fn list_consents<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<ConsentRecord>> {
        match self.outcome("list_consents") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_consents(realm_id, user_id),
        }
    }

    fn delete_consent<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_consent") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.delete_consent(realm_id, user_id, client_id),
        }
    }

    fn upsert_required_action<'a>(&'a self, ra: RequiredActionConfig) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_required_action") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_required_action(ra),
        }
    }

    fn list_required_actions<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<RequiredActionConfig>> {
        match self.outcome("list_required_actions") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_required_actions(realm_id),
        }
    }

    fn upsert_branding<'a>(&'a self, branding: BrandingBlock) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_branding") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_branding(branding),
        }
    }

    fn get_branding<'a>(
        &'a self,
        realm_id: &'a str,
        org_id: Option<&'a str>,
    ) -> StoreFuture<'a, Option<BrandingBlock>> {
        match self.outcome("get_branding") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.get_branding(realm_id, org_id),
        }
    }

    fn append_event<'a>(&'a self, event: AuditEvent) -> StoreFuture<'a, ()> {
        match self.outcome("append_event") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.append_event(event),
        }
    }

    fn list_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AuditEvent>> {
        match self.outcome("list_events") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_events(realm_id, allowed_orgs),
        }
    }

    fn append_admin_event<'a>(&'a self, event: AdminEvent) -> StoreFuture<'a, ()> {
        match self.outcome("append_admin_event") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.append_admin_event(event),
        }
    }

    fn list_admin_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AdminEvent>> {
        match self.outcome("list_admin_events") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_admin_events(realm_id, allowed_orgs),
        }
    }

    fn insert_webauthn_credential<'a>(
        &'a self,
        cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, WebAuthnCredentialRecord> {
        match self.outcome("insert_webauthn_credential") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.insert_webauthn_credential(cred),
        }
    }

    fn update_webauthn_credential<'a>(
        &'a self,
        cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, ()> {
        match self.outcome("update_webauthn_credential") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.update_webauthn_credential(cred),
        }
    }

    fn get_webauthn_by_credential_id<'a>(
        &'a self,
        realm_id: &'a str,
        credential_id: &'a str,
    ) -> StoreFuture<'a, Option<WebAuthnCredentialRecord>> {
        match self.outcome("get_webauthn_by_credential_id") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .get_webauthn_by_credential_id(realm_id, credential_id),
        }
    }

    fn list_webauthn_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<WebAuthnCredentialRecord>> {
        match self.outcome("list_webauthn_credentials") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_webauthn_credentials(realm_id, user_id),
        }
    }

    fn delete_webauthn_credential<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("delete_webauthn_credential") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .delete_webauthn_credential(realm_id, allowed_orgs, id),
        }
    }

    fn update_user_with_outbox<'a>(
        &'a self,
        user: UserRepresentation,
        outbox: EmailOutboxRecord,
    ) -> StoreFuture<'a, UserRepresentation> {
        match self.outcome("update_user_with_outbox") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            _ => self.inner.update_user_with_outbox(user, outbox),
        }
    }

    fn enqueue_outbox<'a>(&'a self, outbox: EmailOutboxRecord) -> StoreFuture<'a, ()> {
        match self.outcome("enqueue_outbox") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.enqueue_outbox(outbox),
        }
    }

    fn claim_outbox_batch<'a>(
        &'a self,
        realm_id: &'a str,
        limit: usize,
        lease: std::time::Duration,
        max_attempts: u32,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        match self.outcome("claim_outbox_batch") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .claim_outbox_batch(realm_id, limit, lease, max_attempts),
        }
    }

    fn complete_outbox_row<'a>(
        &'a self,
        claimed: &'a EmailOutboxRecord,
        sent: bool,
        error: Option<String>,
        next_attempt_at: Option<jc_time::TimestampTz>,
        max_attempts: u32,
    ) -> StoreFuture<'a, bool> {
        match self.outcome("complete_outbox_row") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => {
                self.inner
                    .complete_outbox_row(claimed, sent, error, next_attempt_at, max_attempts)
            }
        }
    }

    fn retry_outbox_row<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, Option<EmailOutboxRecord>> {
        match self.outcome("retry_outbox_row") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.retry_outbox_row(realm_id, allowed_orgs, id),
        }
    }

    fn list_outbox_rows<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        match self.outcome("list_outbox_rows") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_outbox_rows(realm_id, allowed_orgs),
        }
    }

    fn upsert_idp_link<'a>(&'a self, link: IdentityProviderLinkRecord) -> StoreFuture<'a, ()> {
        match self.outcome("upsert_idp_link") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.upsert_idp_link(link),
        }
    }

    fn get_idp_link_by_external<'a>(
        &'a self,
        realm_id: &'a str,
        idp_alias: &'a str,
        external_user_id: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderLinkRecord>> {
        match self.outcome("get_idp_link_by_external") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self
                .inner
                .get_idp_link_by_external(realm_id, idp_alias, external_user_id),
        }
    }

    fn list_idp_links_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderLinkRecord>> {
        match self.outcome("list_idp_links_for_user") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.list_idp_links_for_user(realm_id, user_id),
        }
    }

    fn sweep_expired<'a>(&'a self) -> StoreFuture<'a, u64> {
        match self.outcome("sweep_expired") {
            Some(Outcome::Fail(e)) => Box::pin(async move { Err(e) }),
            Some(Outcome::Empty) => Box::pin(async { Ok(Default::default()) }),
            _ => self.inner.sweep_expired(),
        }
    }
}
