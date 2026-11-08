-- W2, W7, W11 (K9, K11, K12, K13): Forward-only migration 0002.
-- Adds WebAuthn credential storage, upstream IdP account links, domain verification challenge
-- columns, and outbox scheduling/isolation columns with forced RLS and grants to kaisatsu_app.

ALTER TABLE kaisatsu.organization_domains
    ADD COLUMN IF NOT EXISTS id text COLLATE "C",
    ADD COLUMN IF NOT EXISTS challenge_hash text COLLATE "C",
    ADD COLUMN IF NOT EXISTS challenge_expires_at timestamptz,
    ADD COLUMN IF NOT EXISTS doc jsonb NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE kaisatsu.email_outbox
    ADD COLUMN IF NOT EXISTS org_id text COLLATE "C",
    ADD COLUMN IF NOT EXISTS next_attempt_at timestamptz NOT NULL DEFAULT now(),
    ADD COLUMN IF NOT EXISTS last_error text;

DROP POLICY IF EXISTS email_outbox_rls ON kaisatsu.email_outbox;
CREATE POLICY email_outbox_rls ON kaisatsu.email_outbox
    USING (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)));

CREATE INDEX IF NOT EXISTS idx_email_outbox_due
    ON kaisatsu.email_outbox (realm_id, status, next_attempt_at);

CREATE TABLE kaisatsu.webauthn_credential (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    user_handle text NOT NULL COLLATE "C",
    credential_id text NOT NULL COLLATE "C",
    sign_count bigint NOT NULL DEFAULT 0,
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_webauthn_credential_id
    ON kaisatsu.webauthn_credential (realm_id, credential_id);
CREATE INDEX idx_webauthn_user
    ON kaisatsu.webauthn_credential (realm_id, user_id);
CREATE INDEX idx_webauthn_user_handle
    ON kaisatsu.webauthn_credential (realm_id, user_handle);
ALTER TABLE kaisatsu.webauthn_credential ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.webauthn_credential FORCE ROW LEVEL SECURITY;
CREATE POLICY webauthn_credential_rls ON kaisatsu.webauthn_credential
    USING (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)));

CREATE TABLE kaisatsu.idp_links (
    realm_id text NOT NULL COLLATE "C",
    id text NOT NULL COLLATE "C",
    org_id text COLLATE "C",
    user_id text NOT NULL COLLATE "C",
    idp_alias text NOT NULL COLLATE "C",
    external_user_id text NOT NULL COLLATE "C",
    doc jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    seq bigint GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (realm_id, id)
);
CREATE UNIQUE INDEX idx_idp_links_external
    ON kaisatsu.idp_links (realm_id, idp_alias, external_user_id);
CREATE INDEX idx_idp_links_user
    ON kaisatsu.idp_links (realm_id, user_id);
ALTER TABLE kaisatsu.idp_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.idp_links FORCE ROW LEVEL SECURITY;
CREATE POLICY idp_links_rls ON kaisatsu.idp_links
    USING (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)))
    WITH CHECK (realm_id = kaisatsu.current_realm() AND (org_id IS NULL OR kaisatsu.org_visible(org_id)));

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE kaisatsu.webauthn_credential, kaisatsu.idp_links TO kaisatsu_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA kaisatsu TO kaisatsu_app;
