-- The MCP write confirmations (T-7507): an open question with the person's
-- decision, or a spent nonce (`view` IS NULL). Process memory until now, so
-- two replicas disagreed and a restart lost every pending approval.
-- Compatible: the previous release never reads the table.
CREATE TABLE mcp_confirmations (
  tenant_id text NOT NULL,
  id text NOT NULL,
  subject text NOT NULL,
  view text,
  tool text,
  message text,
  decision boolean,
  expires_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, id),
  CHECK ((view IS NULL) = (tool IS NULL) AND (view IS NULL) = (message IS NULL)),
  CHECK (view IS NOT NULL OR decision IS NULL)
);

CREATE INDEX i_mcp_confirmations_expires ON mcp_confirmations (tenant_id, expires_at);

ALTER TABLE mcp_confirmations ENABLE ROW LEVEL SECURITY;
ALTER TABLE mcp_confirmations FORCE ROW LEVEL SECURITY;

-- no escape of 0042: the id the decision page names carries its Tenant
CREATE POLICY tenant_isolation ON mcp_confirmations
  USING (tenant_id = current_setting('antares.tenant', true))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true));
