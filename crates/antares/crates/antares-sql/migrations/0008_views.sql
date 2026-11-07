-- Endpoint views (ADR-0023 rule 10): one document per view under the
-- reserved tenant `antares-views`, written through /q of a `views` build.
-- Same shape and RLS belt as the other doc tables.
CREATE TABLE views (
  tenant_id text NOT NULL,
  id        text NOT NULL,
  doc       jsonb NOT NULL,
  PRIMARY KEY (tenant_id, id)
);
ALTER TABLE views ENABLE ROW LEVEL SECURITY;
ALTER TABLE views FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON views
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
