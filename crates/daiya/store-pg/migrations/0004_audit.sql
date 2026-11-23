-- S9-28 (T-7572): one row per write or decision of daiya that committed, written in the
-- transaction of its change, so a change stands with its row or not at all. Ids, the operation,
-- its kind, the actor's subject id and the decision; no secret, token, query, name or e-mail.
-- A release before it never reads this table and still starts.
SET LOCAL lock_timeout = '10s';
CREATE TABLE IF NOT EXISTS cr_audit (
    org_id UUID NOT NULL,
    id UUID NOT NULL,
    instance_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind ~ '^[A-Z_]{1,64}$'),
    operation TEXT NOT NULL CHECK (length(operation) BETWEEN 1 AND 64),
    actor_id TEXT NOT NULL CHECK (length(actor_id) <= 256),
    target TEXT NOT NULL CHECK (length(target) <= 512),
    decision TEXT NOT NULL CHECK (decision IN ('allow')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (org_id, id)
);
CREATE INDEX IF NOT EXISTS idx_cr_audit_org_time ON cr_audit (org_id, created_at);
ALTER TABLE cr_audit ENABLE ROW LEVEL SECURITY;
ALTER TABLE cr_audit FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS org_isolation ON cr_audit;
CREATE POLICY org_isolation ON cr_audit
    USING (org_id = NULLIF(current_setting('cr.org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('cr.org', true), '')::uuid);
-- Append and read only: the request role never rewrites or removes a row.
REVOKE ALL ON cr_audit FROM PUBLIC;
GRANT SELECT, INSERT ON cr_audit TO daiya_app;
