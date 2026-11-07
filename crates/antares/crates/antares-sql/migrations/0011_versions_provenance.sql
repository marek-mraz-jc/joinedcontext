-- Versions of a measurement and provenance per write (T-4665, platform
-- ADR-N-054 2.1, 2.3 and 2.4).
--
-- attr_instances keeps the CURRENT version of every measurement, so no read
-- of history changes: a correction copies the row it replaces into
-- attr_versions and rewrites the row in place with the next version number.
-- Both new attr_instances columns have a constant default, a catalog change
-- on every partition (no rewrite, no scan), and no new index is built on the
-- history (0009's note, T-4310): attr_versions holds only superseded rows,
-- a small table read by its own index.
ALTER TABLE attr_instances ADD COLUMN IF NOT EXISTS version integer NOT NULL DEFAULT 1;
ALTER TABLE attr_instances ADD COLUMN IF NOT EXISTS provenance jsonb;

CREATE TABLE IF NOT EXISTS attr_versions (
  tenant_id     text NOT NULL,
  entity_id     text NOT NULL,
  attr_id       text NOT NULL,
  instance_id   text NOT NULL,
  dataset_id    text,
  observed_at   timestamptz NOT NULL,
  version       integer NOT NULL,
  data          jsonb NOT NULL,
  provenance    jsonb,
  superseded_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, entity_id, attr_id, instance_id, observed_at, version)
);
CREATE INDEX IF NOT EXISTS i_attr_versions_observed ON attr_versions (observed_at);

-- The same per-command policies as attr_instances (0005): the retention
-- sweep reads and deletes across Tenants under the service escape, and no
-- escape ever reaches an INSERT or an UPDATE.
ALTER TABLE attr_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE attr_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON attr_versions FOR SELECT
  USING (tenant_id = current_setting('antares.tenant', true)
         OR current_setting('antares.service', true) = 'on');
CREATE POLICY tenant_reap ON attr_versions FOR DELETE
  USING (tenant_id = current_setting('antares.tenant', true)
         OR current_setting('antares.service', true) = 'on');
CREATE POLICY tenant_write ON attr_versions FOR INSERT
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_update ON attr_versions FOR UPDATE
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
