-- W2: kaisatsu PostgreSQL schema, forced RLS on every table, leakproof btree indexes.
-- Migrations run as `kaisatsu_migrate` (schema owner); requests run as `kaisatsu_app`
-- (no BYPASSRLS, owns no tables, grants only on schema `kaisatsu`).

CREATE SCHEMA IF NOT EXISTS kaisatsu;

-- Helper function checking whether an organization-owned row's org_id is visible to the transaction.
-- `kaisatsu.orgs` is either `*` (realm-wide caller) or a comma-delimited list `,org1,org2,`.
CREATE OR REPLACE FUNCTION kaisatsu.org_visible(row_org text)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
    SELECT current_setting('kaisatsu.orgs', true) = '*'
        OR (
            row_org IS NOT NULL
            AND position(',' || row_org || ',' IN ',' || coalesce(current_setting('kaisatsu.orgs', true), '') || ',') > 0
        );
$$;

CREATE TABLE IF NOT EXISTS kaisatsu.realms (
    realm_id text PRIMARY KEY COLLATE "C",
    doc jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE kaisatsu.realms ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.realms FORCE ROW LEVEL SECURITY;
CREATE POLICY realms_rls ON kaisatsu.realms
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.organizations (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    alias_lower text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, org_id)
);
CREATE INDEX IF NOT EXISTS idx_organizations_alias ON kaisatsu.organizations (realm_id, alias_lower);
ALTER TABLE kaisatsu.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.organizations FORCE ROW LEVEL SECURITY;
CREATE POLICY organizations_rls ON kaisatsu.organizations
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.organization_domains (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    domain_lower text NOT NULL COLLATE "C",
    verified boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, domain_lower)
);
ALTER TABLE kaisatsu.organization_domains ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.organization_domains FORCE ROW LEVEL SECURITY;
CREATE POLICY organization_domains_rls ON kaisatsu.organization_domains
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.people (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    username_lower text NOT NULL COLLATE "C",
    email_lower text COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
-- Leakproof btree indexes for hot predicates under forced RLS (W2)
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_username_lower ON kaisatsu.people (realm_id, username_lower);
CREATE INDEX IF NOT EXISTS idx_people_email_lower ON kaisatsu.people (realm_id, email_lower);
ALTER TABLE kaisatsu.people ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.people FORCE ROW LEVEL SECURITY;
CREATE POLICY people_rls ON kaisatsu.people
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.memberships (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    person_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_memberships_person ON kaisatsu.memberships (realm_id, person_id);
ALTER TABLE kaisatsu.memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.memberships FORCE ROW LEVEL SECURITY;
CREATE POLICY memberships_rls ON kaisatsu.memberships
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.credentials (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    credential_type text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_credentials_user ON kaisatsu.credentials (realm_id, user_id);
ALTER TABLE kaisatsu.credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY credentials_rls ON kaisatsu.credentials
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.groups (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    path text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_path ON kaisatsu.groups (realm_id, path);
ALTER TABLE kaisatsu.groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.groups FORCE ROW LEVEL SECURITY;
CREATE POLICY groups_rls ON kaisatsu.groups
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.group_members (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    group_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_group_members_user ON kaisatsu.group_members (realm_id, user_id);
CREATE INDEX IF NOT EXISTS idx_group_members_group ON kaisatsu.group_members (realm_id, group_id);
ALTER TABLE kaisatsu.group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.group_members FORCE ROW LEVEL SECURITY;
CREATE POLICY group_members_rls ON kaisatsu.group_members
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.roles (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    container_id text COLLATE "C",
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.roles FORCE ROW LEVEL SECURITY;
CREATE POLICY roles_rls ON kaisatsu.roles
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.role_mappings (
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
ALTER TABLE kaisatsu.role_mappings ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.role_mappings FORCE ROW LEVEL SECURITY;
CREATE POLICY role_mappings_rls ON kaisatsu.role_mappings
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.applications (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_applications_client_id ON kaisatsu.applications (realm_id, client_id);
ALTER TABLE kaisatsu.applications ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.applications FORCE ROW LEVEL SECURITY;
CREATE POLICY applications_rls ON kaisatsu.applications
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.client_scopes (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.client_scopes ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.client_scopes FORCE ROW LEVEL SECURITY;
CREATE POLICY client_scopes_rls ON kaisatsu.client_scopes
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.identity_providers (
    realm_id text NOT NULL COLLATE "C",
    internal_id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    alias text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, internal_id)
);
ALTER TABLE kaisatsu.identity_providers ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.identity_providers FORCE ROW LEVEL SECURITY;
CREATE POLICY identity_providers_rls ON kaisatsu.identity_providers
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.signing_keys (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    kid text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    encrypted_private_key bytea NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.signing_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.signing_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY signing_keys_rls ON kaisatsu.signing_keys
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.auth_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX IF NOT EXISTS idx_auth_session_token_hash ON kaisatsu.auth_session (realm_id, token_hash);
ALTER TABLE kaisatsu.auth_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.auth_session FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_session_rls ON kaisatsu.auth_session
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.sso_session (
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
CREATE UNIQUE INDEX IF NOT EXISTS idx_sso_session_token_hash ON kaisatsu.sso_session (realm_id, token_hash);
CREATE INDEX IF NOT EXISTS idx_sso_session_user ON kaisatsu.sso_session (realm_id, user_id);
ALTER TABLE kaisatsu.sso_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.sso_session FORCE ROW LEVEL SECURITY;
CREATE POLICY sso_session_rls ON kaisatsu.sso_session
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.client_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    sso_session_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.client_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.client_session FORCE ROW LEVEL SECURITY;
CREATE POLICY client_session_rls ON kaisatsu.client_session
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.offline_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.offline_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.offline_session FORCE ROW LEVEL SECURITY;
CREATE POLICY offline_session_rls ON kaisatsu.offline_session
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.refresh_token_family (
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
CREATE INDEX IF NOT EXISTS idx_refresh_family_token_hash ON kaisatsu.refresh_token_family (realm_id, token_hash);
ALTER TABLE kaisatsu.refresh_token_family ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.refresh_token_family FORCE ROW LEVEL SECURITY;
CREATE POLICY refresh_token_family_rls ON kaisatsu.refresh_token_family
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.revoked_token (
    realm_id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, token_hash)
);
CREATE INDEX IF NOT EXISTS idx_revoked_token_hash ON kaisatsu.revoked_token (realm_id, token_hash);
ALTER TABLE kaisatsu.revoked_token ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.revoked_token FORCE ROW LEVEL SECURITY;
CREATE POLICY revoked_token_rls ON kaisatsu.revoked_token
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.login_failure (
    realm_id text NOT NULL COLLATE "C",
    target_key text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, target_key)
);
ALTER TABLE kaisatsu.login_failure ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.login_failure FORCE ROW LEVEL SECURITY;
CREATE POLICY login_failure_rls ON kaisatsu.login_failure
    USING (realm_id = current_setting('kaisatsu.realm', true))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true));

CREATE TABLE IF NOT EXISTS kaisatsu.consent (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.consent ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.consent FORCE ROW LEVEL SECURITY;
CREATE POLICY consent_rls ON kaisatsu.consent
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.events (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.events FORCE ROW LEVEL SECURITY;
CREATE POLICY events_rls ON kaisatsu.events
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.admin_events (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.admin_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.admin_events FORCE ROW LEVEL SECURITY;
CREATE POLICY admin_events_rls ON kaisatsu.admin_events
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));

CREATE TABLE IF NOT EXISTS kaisatsu.branding_blocks (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.branding_blocks ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.branding_blocks FORCE ROW LEVEL SECURITY;
CREATE POLICY branding_blocks_rls ON kaisatsu.branding_blocks
    USING (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = current_setting('kaisatsu.realm', true) AND kaisatsu.org_visible(org_id));
