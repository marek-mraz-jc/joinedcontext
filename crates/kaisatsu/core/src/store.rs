#![forbid(unsafe_code)]
//! Unified `Store` trait implemented by both `kaisatsu-store-pg` and `kaisatsu-store-memory` (W2).

use crate::authz::{Authorizer, RequestCtx};
use crate::branding::BrandingBlock;
use crate::error::Result;
use crate::model::*;
use futures_core::future::BoxFuture;
use odata_table::{Answer, QueryOptions};

pub type StoreFuture<'a, T> = BoxFuture<'a, Result<T>>;

/// Storage interface for kaisatsu (W2, W4).
pub trait Store: Send + Sync {
    /// Queries any of the 23 OData entity sets using `odata-table` with tenant `Scope` applied before `$filter` (W4).
    fn query_entity_set<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        authorizer: &'a dyn Authorizer,
        set_name: &'a str,
        options: &'a QueryOptions,
        max_page: u64,
    ) -> StoreFuture<'a, Answer>;

    // Realms
    fn get_realm<'a>(&'a self, realm_name: &'a str) -> StoreFuture<'a, Option<RealmConfig>>;
    fn upsert_realm<'a>(&'a self, realm: RealmConfig) -> StoreFuture<'a, ()>;
    fn list_realms<'a>(&'a self) -> StoreFuture<'a, Vec<RealmConfig>>;

    // Organizations & Domains
    fn upsert_organization<'a>(&'a self, org: Organization) -> StoreFuture<'a, ()>;
    fn get_organization<'a>(
        &'a self,
        realm_id: &'a str,
        org_id_or_alias: &'a str,
    ) -> StoreFuture<'a, Option<Organization>>;
    fn list_organizations<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<Organization>>;
    fn find_org_by_domain<'a>(
        &'a self,
        realm_id: &'a str,
        domain: &'a str,
    ) -> StoreFuture<'a, Option<Organization>>;
    fn upsert_membership<'a>(&'a self, m: Membership) -> StoreFuture<'a, ()>;
    fn list_memberships_for_person<'a>(
        &'a self,
        realm_id: &'a str,
        person_id: &'a str,
    ) -> StoreFuture<'a, Vec<Membership>>;

    // People (Users)
    fn create_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation>;
    fn update_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation>;
    fn delete_user<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, bool>;
    fn get_user_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>>;
    fn get_user_by_username_or_email<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        identifier: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>>;
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
    ) -> StoreFuture<'a, Vec<UserRepresentation>>;

    // Credentials
    fn upsert_credential<'a>(&'a self, cred: CredentialRecord) -> StoreFuture<'a, ()>;
    fn list_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<CredentialRecord>>;
    fn delete_credential<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        cred_id: &'a str,
    ) -> StoreFuture<'a, bool>;

    // Groups
    fn upsert_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation>;
    fn get_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>>;
    fn get_group_by_path<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        path: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>>;
    fn list_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        exact: bool,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>>;
    fn delete_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, bool>;
    fn add_user_to_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()>;
    fn remove_user_from_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()>;
    fn list_user_groups<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>>;
    fn list_group_members<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>>;

    // Roles & Role Mappings
    fn upsert_role<'a>(&'a self, role: RoleRepresentation) -> StoreFuture<'a, RoleRepresentation>;
    fn list_roles<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleRepresentation>>;
    fn delete_role<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, bool>;
    fn add_role_mapping<'a>(&'a self, mapping: RoleMappingRecord) -> StoreFuture<'a, ()>;
    fn remove_role_mapping<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, ()>;
    fn list_role_mappings_for_principal<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleMappingRecord>>;
    fn list_principals_with_client_role<'a>(
        &'a self,
        realm_id: &'a str,
        client_uuid: &'a str,
        role_name: &'a str,
        principal_kind: &'a str,
    ) -> StoreFuture<'a, Vec<String>>;

    // Clients (Applications) & Scopes
    fn upsert_client<'a>(
        &'a self,
        client: ClientRepresentation,
    ) -> StoreFuture<'a, ClientRepresentation>;
    fn get_client_by_uuid<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>>;
    fn get_client_by_client_id<'a>(
        &'a self,
        realm_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>>;
    fn list_clients<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        client_id_filter: Option<&'a str>,
        search: bool,
    ) -> StoreFuture<'a, Vec<ClientRepresentation>>;
    fn delete_client<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, bool>;
    fn upsert_client_scope<'a>(
        &'a self,
        scope: ClientScopeRepresentation,
    ) -> StoreFuture<'a, ClientScopeRepresentation>;
    fn get_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientScopeRepresentation>>;
    fn list_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>>;
    fn delete_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, bool>;

    // Identity Providers
    fn upsert_idp<'a>(&'a self, idp: IdentityProviderRepresentation) -> StoreFuture<'a, ()>;
    fn get_idp<'a>(
        &'a self,
        realm_id: &'a str,
        alias: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderRepresentation>>;
    fn list_idps<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderRepresentation>>;

    // Signing Keys
    fn upsert_signing_key<'a>(&'a self, key: SigningKeyRecord) -> StoreFuture<'a, ()>;
    fn list_signing_keys<'a>(&'a self, realm_id: &'a str)
    -> StoreFuture<'a, Vec<SigningKeyRecord>>;

    // Sessions, Codes, PAR, Refresh Families, Revocations, Brute Force, Consents
    fn put_auth_session<'a>(&'a self, session: AuthSession) -> StoreFuture<'a, ()>;
    fn get_auth_session_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
    ) -> StoreFuture<'a, Option<AuthSession>>;
    fn delete_auth_session<'a>(&'a self, realm_id: &'a str, id: &'a str) -> StoreFuture<'a, ()>;

    fn put_auth_code<'a>(&'a self, code: AuthorizationCodeRecord) -> StoreFuture<'a, ()>;
    /// Atomically marks the code as consumed. If it was already consumed, returns `Ok((Some(record), true))`
    /// so the caller can revoke tokens issued from it (W12).
    fn consume_auth_code<'a>(
        &'a self,
        realm_id: &'a str,
        code_hash: &'a str,
    ) -> StoreFuture<'a, (Option<AuthorizationCodeRecord>, bool)>;

    fn put_par_request<'a>(&'a self, par: ParRequestRecord) -> StoreFuture<'a, ()>;
    fn consume_par_request<'a>(
        &'a self,
        realm_id: &'a str,
        request_uri_hash: &'a str,
    ) -> StoreFuture<'a, Option<ParRequestRecord>>;

    fn upsert_sso_session<'a>(&'a self, session: SsoSession) -> StoreFuture<'a, ()>;
    fn get_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>>;
    fn get_sso_session_by_handle<'a>(
        &'a self,
        realm_id: &'a str,
        handle_hash: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>>;
    fn list_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>>;
    fn delete_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>>;
    fn delete_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>>;

    fn upsert_offline_session<'a>(&'a self, session: OfflineSession) -> StoreFuture<'a, ()>;
    fn delete_user_client_offline_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, ()>;

    fn upsert_refresh_family<'a>(&'a self, family: RefreshTokenFamily) -> StoreFuture<'a, ()>;
    fn get_refresh_family<'a>(
        &'a self,
        realm_id: &'a str,
        family_id: &'a str,
    ) -> StoreFuture<'a, Option<RefreshTokenFamily>>;
    /// Replaces `family` only while the stored family is not revoked and its current handle hash
    /// is still `expected_hash` (rotation compare-and-swap). `false` means another request
    /// rotated or revoked it first.
    fn rotate_refresh_family<'a>(
        &'a self,
        family: RefreshTokenFamily,
        expected_hash: &'a str,
    ) -> StoreFuture<'a, bool>;
    fn revoke_refresh_families_for_session<'a>(
        &'a self,
        realm_id: &'a str,
        sso_session_id: &'a str,
        client_id: Option<&'a str>,
    ) -> StoreFuture<'a, ()>;

    fn mark_token_revoked<'a>(&'a self, rec: RevokedTokenRecord) -> StoreFuture<'a, ()>;
    fn is_token_revoked<'a>(
        &'a self,
        realm_id: &'a str,
        jti_hash: &'a str,
    ) -> StoreFuture<'a, bool>;

    fn get_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, Option<LoginFailureRecord>>;
    fn upsert_login_failure<'a>(&'a self, rec: LoginFailureRecord) -> StoreFuture<'a, ()>;
    fn clear_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, ()>;

    fn upsert_consent<'a>(&'a self, consent: ConsentRecord) -> StoreFuture<'a, ()>;
    fn list_consents<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<ConsentRecord>>;
    fn delete_consent<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, bool>;

    // Required Actions & Branding
    fn upsert_required_action<'a>(&'a self, ra: RequiredActionConfig) -> StoreFuture<'a, ()>;
    fn list_required_actions<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<RequiredActionConfig>>;
    fn upsert_branding<'a>(&'a self, branding: BrandingBlock) -> StoreFuture<'a, ()>;
    fn get_branding<'a>(
        &'a self,
        realm_id: &'a str,
        org_id: Option<&'a str>,
    ) -> StoreFuture<'a, Option<BrandingBlock>>;

    // Events
    fn append_event<'a>(&'a self, event: AuditEvent) -> StoreFuture<'a, ()>;
    fn list_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AuditEvent>>;
    fn append_admin_event<'a>(&'a self, event: AdminEvent) -> StoreFuture<'a, ()>;
    fn list_admin_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AdminEvent>>;

    // Sweeper
    fn sweep_expired<'a>(&'a self) -> StoreFuture<'a, u64>;
}
