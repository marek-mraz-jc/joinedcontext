-- S6-20 (T-7571): one row per forge write that committed, written in the transaction of its
-- change, so a change stands with its row or not at all. Ids, the operation, its kind, the actor's
-- subject id and the decision; no secret, file body, name or e-mail.
CREATE TABLE IF NOT EXISTS shako.audit (
    id UUID PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind ~ '^[A-Z_]{1,64}$'),
    operation TEXT NOT NULL CHECK (length(operation) BETWEEN 1 AND 64),
    actor_id TEXT NOT NULL CHECK (length(actor_id) <= 256),
    target TEXT NOT NULL CHECK (length(target) <= 512),
    decision TEXT NOT NULL CHECK (decision IN ('allow')),
    request_id TEXT NOT NULL CHECK (length(request_id) <= 128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_shako_audit_tenant ON shako.audit (instance_id, org_id, created_at);

ALTER TABLE shako.audit ENABLE ROW LEVEL SECURITY;
ALTER TABLE shako.audit FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON shako.audit;
CREATE POLICY tenant_isolation ON shako.audit
    USING (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    )
    WITH CHECK (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    );
-- Append and read only: the request role never rewrites or removes a row.
GRANT SELECT, INSERT ON shako.audit TO shako_app;
