-- Every store mutation in order, per tenant `(instance_id, org_id)`. The request path reads the
-- in-memory mirror; `PgStore::hydrate` replays a tenant's log into it at start, so pull requests,
-- reviews, webhooks, packages and the rest survive a restart like the typed tables do.
CREATE TABLE IF NOT EXISTS shako.store_log (
    seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    op JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_shako_store_log_tenant ON shako.store_log (instance_id, org_id, seq);

ALTER TABLE shako.store_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE shako.store_log FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON shako.store_log;
CREATE POLICY tenant_isolation ON shako.store_log
    USING (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    )
    WITH CHECK (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    );
-- Append and read only: the log is never rewritten by the request role.
GRANT SELECT, INSERT ON shako.store_log TO shako_app;
