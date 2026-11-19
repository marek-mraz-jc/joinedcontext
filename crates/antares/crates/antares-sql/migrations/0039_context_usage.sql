-- The usage counters of a Cached @context, per Tenant (S5-10; clause 5.13.3, numberOfHits).
--
-- 0006 left one `jsonld_contexts` row per external URL, readable by every
-- Tenant, with its `numberOfHits` and `lastUsage` inside the shared body: any
-- Tenant listing /jsonldContexts saw which @context URLs the other Tenants
-- named (hostnames, paths, query-string tokens), how often and when. The body
-- of a Cached copy stays shared, since one row per URL is what a cache is for;
-- who used it moves here, one row per (Tenant, copy), behind the same
-- Row-Level Security every Tenant table wears. A Tenant lists a Cached copy
-- only when it holds a row here.
--
-- `tenant_id` is a plain column, so `tenant_tables()` (0037) lists the table
-- and a Tenant rename moves its rows; the purge deletes them by name. Evicting
-- or deleting the copy takes every Tenant's counters with it by the foreign
-- key.
CREATE TABLE jsonld_context_usage (
  context_id text NOT NULL REFERENCES jsonld_contexts (id) ON DELETE CASCADE,
  tenant_id text NOT NULL,
  hits bigint NOT NULL DEFAULT 0,
  last_usage text,
  PRIMARY KEY (context_id, tenant_id)
);

CREATE INDEX i_jsonld_context_usage_tenant ON jsonld_context_usage (tenant_id);

-- Split by command for the reason 0005 records; no NULL Tenant and no
-- `antares.service` escape: every row is one Tenant's.
ALTER TABLE jsonld_context_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE jsonld_context_usage FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_read ON jsonld_context_usage FOR SELECT
  USING (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_write ON jsonld_context_usage FOR INSERT
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_update ON jsonld_context_usage FOR UPDATE
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
CREATE POLICY tenant_reap ON jsonld_context_usage FOR DELETE
  USING (tenant_id = current_setting('antares.tenant', true));

-- The shared counters counted every Tenant's uses together and no row says
-- whose they were, so none is handed to any Tenant: a copy is listed again
-- to each Tenant from its next use.
UPDATE jsonld_contexts SET body = body - 'numberOfHits' - 'lastUsage'
 WHERE kind = 'Cached';
