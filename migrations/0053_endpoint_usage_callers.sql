-- The kind of caller per usage row (T-6186): person, workload, agent (an MCP connector or the
-- assistant), anonymous, as the gateway reads it from a token the realm signed. A kind, never a
-- subject, client id or address. Rows counted before the split keep `unknown`. The surface check
-- gains `feed`, which the gateway counts and an older check refused, failing the whole post.
ALTER TABLE endpoint_usage_daily
  ADD COLUMN IF NOT EXISTS caller text NOT NULL DEFAULT 'unknown'
    CHECK (caller IN ('person', 'workload', 'agent', 'anonymous', 'unknown'));
ALTER TABLE endpoint_usage_daily DROP CONSTRAINT IF EXISTS endpoint_usage_daily_surface_check;
ALTER TABLE endpoint_usage_daily ADD CONSTRAINT endpoint_usage_daily_surface_check
  CHECK (surface IN ('ngsi-ld', 'ogc', 'odata', 'sta', 'ical', 'feed', 'mcp', 'websocket', 'download'));
ALTER TABLE endpoint_usage_daily DROP CONSTRAINT IF EXISTS endpoint_usage_daily_pkey;
ALTER TABLE endpoint_usage_daily ADD PRIMARY KEY (project, endpoint, day, surface, caller);
