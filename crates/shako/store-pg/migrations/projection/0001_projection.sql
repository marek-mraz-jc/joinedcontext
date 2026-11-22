-- T-7508: the typed tables hold what is served. Every write still appends to shako.store_log, and
-- shako.projection says how much of a tenant's log the tables hold, so the log a previous release
-- wrote (the first start, or after a rollback) is projected before that tenant is served.
-- This file has a ledger of its own (shako_projection_migrations): a previous release refuses to
-- start on a version it does not know in shako_migrations, and nothing here stops it working.
ALTER TABLE shako.webhook ADD COLUMN IF NOT EXISTS format TEXT NOT NULL DEFAULT 'gitea';
ALTER TABLE shako.push_mirror ADD COLUMN IF NOT EXISTS branch TEXT NOT NULL DEFAULT '';
-- The log's sequence, the order these are served and dispatched in; 0 where no log entry made the
-- row (an Organization restore, or a release before the log).
ALTER TABLE shako.review ADD COLUMN IF NOT EXISTS position BIGINT NOT NULL DEFAULT 0;
ALTER TABLE shako.delivery ADD COLUMN IF NOT EXISTS position BIGINT NOT NULL DEFAULT 0;
ALTER TABLE shako.outbox ADD COLUMN IF NOT EXISTS position BIGINT NOT NULL DEFAULT 0;
ALTER TABLE shako.outbox ADD COLUMN IF NOT EXISTS claimed_until TIMESTAMPTZ;

-- No release wrote these four tables before this one. Their keys lacked org_id and named the
-- parent by id where the Store names it by owner and name.
ALTER TABLE shako.package DROP CONSTRAINT IF EXISTS package_instance_id_owner_package_type_name_key;
ALTER TABLE shako.package_version DROP CONSTRAINT IF EXISTS package_version_instance_id_package_id_version_key;
ALTER TABLE shako.package_file DROP CONSTRAINT IF EXISTS package_file_instance_id_version_id_filename_key;
ALTER TABLE shako.blob DROP CONSTRAINT IF EXISTS blob_instance_id_owner_repo_name_digest_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_package_key
    ON shako.package (instance_id, org_id, owner, package_type, name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_package_version_key
    ON shako.package_version (instance_id, org_id, owner, package_type, package_name, version);
CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_package_file_key
    ON shako.package_file (instance_id, org_id, owner, package_name, version, filename);
CREATE UNIQUE INDEX IF NOT EXISTS idx_shako_blob_key
    ON shako.blob (instance_id, org_id, owner, repo_name, digest);
CREATE INDEX IF NOT EXISTS idx_shako_webhook_repo ON shako.webhook (instance_id, org_id, repo_id);
CREATE INDEX IF NOT EXISTS idx_shako_delivery_hook ON shako.delivery (instance_id, org_id, webhook_id);
CREATE INDEX IF NOT EXISTS idx_shako_outbox_pending
    ON shako.outbox (instance_id, org_id, position, created_at, id) WHERE NOT dispatched;

CREATE TABLE IF NOT EXISTS shako.projection (
    instance_id UUID NOT NULL,
    org_id UUID NOT NULL,
    seq BIGINT NOT NULL,
    PRIMARY KEY (instance_id, org_id)
);
ALTER TABLE shako.projection ENABLE ROW LEVEL SECURITY;
ALTER TABLE shako.projection FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON shako.projection;
CREATE POLICY tenant_isolation ON shako.projection
    USING (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    )
    WITH CHECK (
        instance_id = NULLIF(current_setting('shako.instance', true), '')::uuid
        AND org_id = NULLIF(current_setting('shako.org', true), '')::uuid
    );
GRANT SELECT, INSERT, UPDATE ON shako.projection TO shako_app;
