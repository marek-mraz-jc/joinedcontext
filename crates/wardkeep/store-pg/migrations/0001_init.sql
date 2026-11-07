-- W2: wardkeep PostgreSQL schema, forced RLS on every table, leakproof btree indexes.
-- Migrations run as `wardkeep_migrate` (schema owner); requests run as `wardkeep_app`
-- (no BYPASSRLS, owns no tables, grants only on schema `wardkeep`).

CREATE SCHEMA IF NOT EXISTS wardkeep;

-- Helper function checking whether an organization-owned row's org_id is visible to the transaction.
-- `wardkeep.orgs` is either `*` (realm-wide caller) or a comma-delimited list `,org1,org2,`.
CREATE OR REPLACE FUNCTION wardkeep.org_visible(row_org text)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
    SELECT current_setting('wardkeep.orgs', true) = '*'
        OR (
            row_org IS NOT NULL
            AND position(',' || row_org || ',' IN ',' || coalesce(current_setting('wardkeep.orgs', true), '') || ',') > 0
        );
$$;

CREATE TABLE IF NOT EXISTS wardkeep.realms (
    realm_id text PRIMARY KEY COLLATE "C",
    doc jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE wardkeep.realms ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.realms FORCE ROW LEVEL SECURITY;
CREATE POLICY realms_rls ON wardkeep.realms
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.organizations (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    alias_lower text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, org_id)
);
CREATE INDEX IF NOT EXISTS idx_organizations_alias ON wardkeep.organizations (realm_id, alias_lower);
ALTER TABLE wardkeep.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.organizations FORCE ROW LEVEL SECURITY;
CREATE POLICY organizations_rls ON wardkeep.organizations
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.organization_domains (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    domain_lower text NOT NULL COLLATE "C",
    verified boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, domain_lower)
);
ALTER TABLE wardkeep.organization_domains ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.organization_domains FORCE ROW LEVEL SECURITY;
CREATE POLICY organization_domains_rls ON wardkeep.organization_domains
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.people (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    username_lower text NOT NULL COLLATE "C",
    email_lower text COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
-- Leakproof btree indexes for hot predicates under forced RLS (W2)
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_username_lower ON wardkeep.people (realm_id, username_lower);
CREATE INDEX IF NOT EXISTS idx_people_email_lower ON wardkeep.people (realm_id, email_lower);
ALTER TABLE wardkeep.people ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.people FORCE ROW LEVEL SECURITY;
CREATE POLICY people_rls ON wardkeep.people
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.memberships (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    person_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_memberships_person ON wardkeep.memberships (realm_id, person_id);
ALTER TABLE wardkeep.memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.memberships FORCE ROW LEVEL SECURITY;
CREATE POLICY memberships_rls ON wardkeep.memberships
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.credentials (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    credential_type text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_credentials_user ON wardkeep.credentials (realm_id, user_id);
ALTER TABLE wardkeep.credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY credentials_rls ON wardkeep.credentials
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.groups (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    path text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_path ON wardkeep.groups (realm_id, path);
ALTER TABLE wardkeep.groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.groups FORCE ROW LEVEL SECURITY;
CREATE POLICY groups_rls ON wardkeep.groups
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.group_members (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    group_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_group_members_user ON wardkeep.group_members (realm_id, user_id);
CREATE INDEX IF NOT EXISTS idx_group_members_group ON wardkeep.group_members (realm_id, group_id);
ALTER TABLE wardkeep.group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.group_members FORCE ROW LEVEL SECURITY;
CREATE POLICY group_members_rls ON wardkeep.group_members
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.roles (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    container_id text COLLATE "C",
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.roles FORCE ROW LEVEL SECURITY;
CREATE POLICY roles_rls ON wardkeep.roles
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.role_mappings (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    principal_kind text NOT NULL COLLATE "C",
    principal_id text NOT NULL COLLATE "C",
    client_uuid text COLLATE "C",
    role_name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.role_mappings ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.role_mappings FORCE ROW LEVEL SECURITY;
CREATE POLICY role_mappings_rls ON wardkeep.role_mappings
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.applications (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_applications_client_id ON wardkeep.applications (realm_id, client_id);
ALTER TABLE wardkeep.applications ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.applications FORCE ROW LEVEL SECURITY;
CREATE POLICY applications_rls ON wardkeep.applications
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.client_scopes (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.client_scopes ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.client_scopes FORCE ROW LEVEL SECURITY;
CREATE POLICY client_scopes_rls ON wardkeep.client_scopes
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.identity_providers (
    realm_id text NOT NULL COLLATE "C",
    internal_id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    alias text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, internal_id)
);
ALTER TABLE wardkeep.identity_providers ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.identity_providers FORCE ROW LEVEL SECURITY;
CREATE POLICY identity_providers_rls ON wardkeep.identity_providers
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.signing_keys (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    kid text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    encrypted_private_key bytea NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.signing_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.signing_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY signing_keys_rls ON wardkeep.signing_keys
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.auth_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_auth_session_token_hash ON wardkeep.auth_session (realm_id, token_hash);
ALTER TABLE wardkeep.auth_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.auth_session FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_session_rls ON wardkeep.auth_session
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.sso_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, id)
);
-- Leakproof btree index on (realm_id, token_hash) (W2)
CREATE UNIQUE INDEX IF NOT EXISTS idx_sso_session_token_hash ON wardkeep.sso_session (realm_id, token_hash);
CREATE INDEX IF NOT EXISTS idx_sso_session_user ON wardkeep.sso_session (realm_id, user_id);
ALTER TABLE wardkeep.sso_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.sso_session FORCE ROW LEVEL SECURITY;
CREATE POLICY sso_session_rls ON wardkeep.sso_session
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.client_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    sso_session_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.client_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.client_session FORCE ROW LEVEL SECURITY;
CREATE POLICY client_session_rls ON wardkeep.client_session
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.offline_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.offline_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.offline_session FORCE ROW LEVEL SECURITY;
CREATE POLICY offline_session_rls ON wardkeep.offline_session
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.refresh_token_family (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    sso_session_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_refresh_family_token_hash ON wardkeep.refresh_token_family (realm_id, token_hash);
ALTER TABLE wardkeep.refresh_token_family ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.refresh_token_family FORCE ROW LEVEL SECURITY;
CREATE POLICY refresh_token_family_rls ON wardkeep.refresh_token_family
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.revoked_token (
    realm_id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, token_hash)
);
CREATE INDEX IF NOT EXISTS idx_revoked_token_hash ON wardkeep.revoked_token (realm_id, token_hash);
ALTER TABLE wardkeep.revoked_token ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.revoked_token FORCE ROW LEVEL SECURITY;
CREATE POLICY revoked_token_rls ON wardkeep.revoked_token
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.login_failure (
    realm_id text NOT NULL COLLATE "C",
    target_key text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, target_key)
);
ALTER TABLE wardkeep.login_failure ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.login_failure FORCE ROW LEVEL SECURITY;
CREATE POLICY login_failure_rls ON wardkeep.login_failure
    USING (realm_id = current_setting('wardkeep.realm', true))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true));

CREATE TABLE IF NOT EXISTS wardkeep.consent (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.consent ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.consent FORCE ROW LEVEL SECURITY;
CREATE POLICY consent_rls ON wardkeep.consent
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.events (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.events FORCE ROW LEVEL SECURITY;
CREATE POLICY events_rls ON wardkeep.events
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.admin_events (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.admin_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.admin_events FORCE ROW LEVEL SECURITY;
CREATE POLICY admin_events_rls ON wardkeep.admin_events
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));

CREATE TABLE IF NOT EXISTS wardkeep.branding_blocks (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE wardkeep.branding_blocks ENABLE ROW LEVEL SECURITY;
ALTER TABLE wardkeep.branding_blocks FORCE ROW LEVEL SECURITY;
CREATE POLICY branding_blocks_rls ON wardkeep.branding_blocks
    USING (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('wardkeep.realm', true) AND wardkeep.org_visible(org_id));
