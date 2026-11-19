-- The hook and mirror secrets (S6-09, T-7280), sealed: AES-256-GCM under a subkey of the
-- installation's KEK, the tenant and the reference bound in as associated data, so a row copied to
-- another tenant or reference does not open. A forge-made reference names its repository
-- (`repo:{RepoId}:webhook-…`, `repo:{RepoId}:mirror:…`). No plaintext column.
CREATE TABLE IF NOT EXISTS shako.secret (
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    secret_ref TEXT NOT NULL COLLATE "C",
    sealed BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, org_id, secret_ref)
);

ALTER TABLE shako.secret ENABLE ROW LEVEL SECURITY;
ALTER TABLE shako.secret FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON shako.secret;
CREATE POLICY tenant_isolation ON shako.secret
    USING (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    )
    WITH CHECK (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    );
GRANT SELECT, INSERT, UPDATE ON shako.secret TO shako_app;
