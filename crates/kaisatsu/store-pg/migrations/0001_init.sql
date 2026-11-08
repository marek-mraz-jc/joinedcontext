-- W2: kaisatsu PostgreSQL schema, forced RLS on every table, leakproof btree indexes.
-- Runs once per database through the sqlx migrator (version table
-- kaisatsu_migrations._sqlx_migrations) as `kaisatsu_migrate`, which owns the schema and every
-- table. Requests run as `kaisatsu_app`: no SUPERUSER, no BYPASSRLS, owns nothing, and holds
-- only the grants at the end of this file. Both roles exist before the first migration (the
-- deployment creates them; the tests create them in their throwaway server).
--
-- Every table carries `realm_id`; organization-owned tables carry `org_id`. Each request opens a
-- transaction and sets `kaisatsu.realm` (one realm id) and `kaisatsu.orgs` (`*` for a realm-wide
-- caller, otherwise a JSON array of organization ids) with `SET LOCAL` semantics. A missing
-- setting matches no row. Only `kaisatsu.realms` accepts `kaisatsu.realm = '*'` (realm listing).
-- Each table keeps the serialized record in `doc`; the plain columns carry the keys the
-- policies and the hot lookups need. `seq` keeps insertion order for listings.

CREATE SCHEMA kaisatsu;
REVOKE ALL ON SCHEMA kaisatsu FROM PUBLIC;

-- True when an organization-owned row is visible to the transaction.
CREATE FUNCTION kaisatsu.org_visible(row_org text)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
    SELECT coalesce(current_setting('kaisatsu.orgs', true), '') = '*'
        OR (
            row_org IS NOT NULL
            AND coalesce(current_setting('kaisatsu.orgs', true), '') LIKE '[%'
            AND row_org IN (
                SELECT jsonb_array_elements_text(current_setting('kaisatsu.orgs', true)::jsonb)
            )
        );
$$;

-- The realm of the transaction; NULL (matching nothing) when unset or empty.
CREATE FUNCTION kaisatsu.current_realm()
RETURNS text
LANGUAGE sql
STABLE
AS $$
    SELECT nullif(current_setting('kaisatsu.realm', true), '');
$$;

CREATE TABLE kaisatsu.realms (
    realm_id text PRIMARY KEY COLLATE "C",
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    smtp_password_enc bytea,
    seq bigint GENERATED ALWAYS AS IDENTITY
);
CREATE UNIQUE INDEX idx_realms_name ON kaisatsu.realms (name);
ALTER TABLE kaisatsu.realms ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.realms FORCE ROW LEVEL SECURITY;
CREATE POLICY realms_rls ON kaisatsu.realms
    USING (realm_id = kaisatsu.current_realm() OR kaisatsu.current_realm() = '*')
    WITH CHECK (realm_id = kaisatsu.current_realm() OR kaisatsu.current_realm() = '*');

CREATE TABLE kaisatsu.organizations (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    alias text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, org_id)
);
CREATE UNIQUE INDEX idx_organizations_alias ON kaisatsu.organizations (realm_id, alias);
ALTER TABLE kaisatsu.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.organizations FORCE ROW LEVEL SECURITY;
CREATE POLICY organizations_rls ON kaisatsu.organizations
    USING (realm_id = kaisatsu.current_realm() AND (kaisatsu.org_visible(org_id) OR kaisatsu.org_visible(alias)))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND (kaisatsu.org_visible(org_id) OR kaisatsu.org_visible(alias)));

CREATE TABLE kaisatsu.organization_domains (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    domain_lower text NOT NULL COLLATE "C",
    verified boolean NOT NULL DEFAULT false,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, org_id, domain_lower)
);
CREATE INDEX idx_organization_domains_domain ON kaisatsu.organization_domains (realm_id, domain_lower);
ALTER TABLE kaisatsu.organization_domains ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.organization_domains FORCE ROW LEVEL SECURITY;
CREATE POLICY organization_domains_rls ON kaisatsu.organization_domains
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.memberships (
    realm_id text NOT NULL COLLATE "C",
    org_id text NOT NULL COLLATE "C",
    person_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, org_id, person_id)
);
CREATE INDEX idx_memberships_person ON kaisatsu.memberships (realm_id, person_id);
ALTER TABLE kaisatsu.memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.memberships FORCE ROW LEVEL SECURITY;
CREATE POLICY memberships_rls ON kaisatsu.memberships
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.groups (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    path text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_groups_path ON kaisatsu.groups (realm_id, path);
ALTER TABLE kaisatsu.groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.groups FORCE ROW LEVEL SECURITY;
CREATE POLICY groups_rls ON kaisatsu.groups
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.group_members (
    realm_id text NOT NULL COLLATE "C",
    group_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, group_id, user_id)
);
CREATE INDEX idx_group_members_user ON kaisatsu.group_members (realm_id, user_id);
ALTER TABLE kaisatsu.group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.group_members FORCE ROW LEVEL SECURITY;
CREATE POLICY group_members_rls ON kaisatsu.group_members
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

-- People are global in the realm; an organization-scoped transaction sees a person whose home
-- organization, membership or group (by owner or by the `/{org}/...` path root) is visible.
CREATE TABLE kaisatsu.people (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    username_lower text NOT NULL COLLATE "C",
    email_lower text COLLATE "C",
    transient boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
-- Leakproof btree indexes for hot predicates under forced RLS (W2)
CREATE UNIQUE INDEX idx_people_username_lower ON kaisatsu.people (realm_id, username_lower);
CREATE UNIQUE INDEX idx_people_email_lower ON kaisatsu.people (realm_id, email_lower) WHERE email_lower IS NOT NULL;
ALTER TABLE kaisatsu.people ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.people FORCE ROW LEVEL SECURITY;
CREATE POLICY people_rls ON kaisatsu.people
    USING (
        realm_id = kaisatsu.current_realm()
        AND (
            kaisatsu.org_visible(org_id)
            OR EXISTS (
                SELECT 1 FROM kaisatsu.memberships m
                WHERE m.realm_id = people.realm_id AND m.person_id = people.id
            )
            OR EXISTS (
                SELECT 1 FROM kaisatsu.group_members gm
                JOIN kaisatsu.groups g ON g.realm_id = gm.realm_id AND g.id = gm.group_id
                WHERE gm.realm_id = people.realm_id AND gm.user_id = people.id
            )
        )
    )
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.credentials (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    credential_type text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX idx_credentials_user ON kaisatsu.credentials (realm_id, user_id);
ALTER TABLE kaisatsu.credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY credentials_rls ON kaisatsu.credentials
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.roles (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    container_id text COLLATE "C",
    client_role boolean NOT NULL DEFAULT false,
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_roles_name ON kaisatsu.roles (realm_id, coalesce(container_id, ''), name);
ALTER TABLE kaisatsu.roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.roles FORCE ROW LEVEL SECURITY;
CREATE POLICY roles_rls ON kaisatsu.roles
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.role_mappings (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    principal_kind text NOT NULL COLLATE "C",
    principal_id text NOT NULL COLLATE "C",
    client_uuid text COLLATE "C",
    role_name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_role_mappings_key ON kaisatsu.role_mappings
    (realm_id, principal_kind, principal_id, coalesce(client_uuid, ''), role_name);
ALTER TABLE kaisatsu.role_mappings ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.role_mappings FORCE ROW LEVEL SECURITY;
CREATE POLICY role_mappings_rls ON kaisatsu.role_mappings
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

-- Client secrets are stored encrypted under the KEK (`secret_enc`), never inside `doc`.
CREATE TABLE kaisatsu.applications (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    secret_enc bytea,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_applications_client_id ON kaisatsu.applications (realm_id, client_id);
ALTER TABLE kaisatsu.applications ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.applications FORCE ROW LEVEL SECURITY;
CREATE POLICY applications_rls ON kaisatsu.applications
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.client_scopes (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    name text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_client_scopes_name ON kaisatsu.client_scopes (realm_id, name);
ALTER TABLE kaisatsu.client_scopes ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.client_scopes FORCE ROW LEVEL SECURITY;
CREATE POLICY client_scopes_rls ON kaisatsu.client_scopes
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.identity_providers (
    realm_id text NOT NULL COLLATE "C",
    alias text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, alias)
);
ALTER TABLE kaisatsu.identity_providers ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.identity_providers FORCE ROW LEVEL SECURITY;
CREATE POLICY identity_providers_rls ON kaisatsu.identity_providers
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

-- Private keys stay AES-256-GCM ciphertext under the KEK (W10).
CREATE TABLE kaisatsu.signing_keys (
    realm_id text NOT NULL COLLATE "C",
    kid text NOT NULL COLLATE "C",
    priority integer NOT NULL,
    doc jsonb NOT NULL,
    encrypted_private_key bytea NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, kid)
);
ALTER TABLE kaisatsu.signing_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.signing_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY signing_keys_rls ON kaisatsu.signing_keys
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.auth_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX idx_auth_session_expiry ON kaisatsu.auth_session (expires_at);
ALTER TABLE kaisatsu.auth_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.auth_session FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_session_rls ON kaisatsu.auth_session
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

-- Authorization codes, stored by SHA-256 only (W2, W12).
CREATE TABLE kaisatsu.auth_code (
    realm_id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    consumed boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, token_hash)
);
ALTER TABLE kaisatsu.auth_code ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.auth_code FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_code_rls ON kaisatsu.auth_code
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.par_request (
    realm_id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    consumed boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, token_hash)
);
ALTER TABLE kaisatsu.par_request ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.par_request FORCE ROW LEVEL SECURITY;
CREATE POLICY par_request_rls ON kaisatsu.par_request
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.sso_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
-- Leakproof btree index on (realm_id, token_hash) (W2)
CREATE INDEX idx_sso_session_token_hash ON kaisatsu.sso_session (realm_id, token_hash);
CREATE INDEX idx_sso_session_user ON kaisatsu.sso_session (realm_id, user_id);
ALTER TABLE kaisatsu.sso_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.sso_session FORCE ROW LEVEL SECURITY;
CREATE POLICY sso_session_rls ON kaisatsu.sso_session
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.client_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    sso_session_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_client_session_key ON kaisatsu.client_session (realm_id, sso_session_id, client_id);
ALTER TABLE kaisatsu.client_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.client_session FORCE ROW LEVEL SECURITY;
CREATE POLICY client_session_rls ON kaisatsu.client_session
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.offline_session (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.offline_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.offline_session FORCE ROW LEVEL SECURITY;
CREATE POLICY offline_session_rls ON kaisatsu.offline_session
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

-- Refresh token families: rotation counter, SHA-256 of the current handle, DPoP jkt (W2).
CREATE TABLE kaisatsu.refresh_token_family (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    sso_session_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    revoked boolean NOT NULL DEFAULT false,
    doc jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX idx_refresh_family_token_hash ON kaisatsu.refresh_token_family (realm_id, token_hash);
CREATE INDEX idx_refresh_family_session ON kaisatsu.refresh_token_family (realm_id, sso_session_id);
ALTER TABLE kaisatsu.refresh_token_family ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.refresh_token_family FORCE ROW LEVEL SECURITY;
CREATE POLICY refresh_token_family_rls ON kaisatsu.refresh_token_family
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.revoked_token (
    realm_id text NOT NULL COLLATE "C",
    token_hash text NOT NULL COLLATE "C",
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (realm_id, token_hash)
);
ALTER TABLE kaisatsu.revoked_token ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.revoked_token FORCE ROW LEVEL SECURITY;
CREATE POLICY revoked_token_rls ON kaisatsu.revoked_token
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.login_failure (
    realm_id text NOT NULL COLLATE "C",
    target_key text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    PRIMARY KEY (realm_id, target_key)
);
ALTER TABLE kaisatsu.login_failure ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.login_failure FORCE ROW LEVEL SECURITY;
CREATE POLICY login_failure_rls ON kaisatsu.login_failure
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.consent (
    realm_id text NOT NULL COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    client_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, user_id, client_id)
);
ALTER TABLE kaisatsu.consent ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.consent FORCE ROW LEVEL SECURITY;
CREATE POLICY consent_rls ON kaisatsu.consent
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.required_actions (
    realm_id text NOT NULL COLLATE "C",
    alias text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, alias)
);
ALTER TABLE kaisatsu.required_actions ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.required_actions FORCE ROW LEVEL SECURITY;
CREATE POLICY required_actions_rls ON kaisatsu.required_actions
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

CREATE TABLE kaisatsu.branding_blocks (
    realm_id text NOT NULL COLLATE "C",
    org_key text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, org_key)
);
ALTER TABLE kaisatsu.branding_blocks ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.branding_blocks FORCE ROW LEVEL SECURITY;
CREATE POLICY branding_blocks_rls ON kaisatsu.branding_blocks
    USING (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)));

CREATE TABLE kaisatsu.events (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.events FORCE ROW LEVEL SECURITY;
CREATE POLICY events_rls ON kaisatsu.events
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

CREATE TABLE kaisatsu.admin_events (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    doc jsonb NOT NULL,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.admin_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.admin_events FORCE ROW LEVEL SECURITY;
CREATE POLICY admin_events_rls ON kaisatsu.admin_events
    USING (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND kaisatsu.org_visible(org_id));

-- E-mail outbox (W7): one row per message to send; the sender marks it sent or failed.
CREATE TABLE kaisatsu.email_outbox (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    status text NOT NULL DEFAULT 'pending' COLLATE "C",
    attempts integer NOT NULL DEFAULT 0,
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE INDEX idx_email_outbox_pending ON kaisatsu.email_outbox (realm_id, status);
ALTER TABLE kaisatsu.email_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.email_outbox FORCE ROW LEVEL SECURITY;
CREATE POLICY email_outbox_rls ON kaisatsu.email_outbox
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());

-- The request role: data rights on the schema's tables only, nothing else.
GRANT USAGE ON SCHEMA kaisatsu TO kaisatsu_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA kaisatsu TO kaisatsu_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA kaisatsu TO kaisatsu_app;
GRANT EXECUTE ON FUNCTION kaisatsu.org_visible(text), kaisatsu.current_realm() TO kaisatsu_app;
