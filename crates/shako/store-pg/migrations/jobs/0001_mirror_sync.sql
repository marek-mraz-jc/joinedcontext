-- T-7522: which repositories' push mirrors are behind, and who syncs them now. Job state beside
-- the store log, never replayed from it; a ledger of its own, so an earlier release runs on
-- this schema (STO-11).
CREATE TABLE IF NOT EXISTS shako.mirror_sync (
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    repo_id UUID NOT NULL,
    mark BIGINT NOT NULL DEFAULT 1,
    done BIGINT NOT NULL DEFAULT 0,
    marked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_until TIMESTAMPTZ,
    PRIMARY KEY (instance_id, org_id, repo_id)
);
CREATE INDEX IF NOT EXISTS idx_shako_mirror_sync_due
    ON shako.mirror_sync (instance_id, org_id, marked_at) WHERE mark > done;

ALTER TABLE shako.mirror_sync ENABLE ROW LEVEL SECURITY;
ALTER TABLE shako.mirror_sync FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON shako.mirror_sync;
CREATE POLICY tenant_isolation ON shako.mirror_sync
    USING (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    )
    WITH CHECK (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    );
GRANT SELECT, INSERT, UPDATE ON shako.mirror_sync TO shako_app;
