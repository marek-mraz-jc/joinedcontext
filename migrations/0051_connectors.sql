-- A person's connectors (AG-142, ADR-N-064, T-5950): a grant one person gives one AI client over
-- part of the platform, served as an MCP server at /c/{id}. Kept here, never in Git. `grant_doc`
-- holds segments, scope, writes, expiresAt and allowedClients as the API sends them; the columns
-- are what a query filters on.
CREATE TABLE IF NOT EXISTS connectors (
  id text PRIMARY KEY,
  owner text NOT NULL,
  name text NOT NULL,
  grant_doc jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  last_used_at timestamptz,
  revoked_at timestamptz
);
-- One live connector of a name per person; a revoked one keeps its name for its audit.
CREATE UNIQUE INDEX IF NOT EXISTS connectors_owner_name
  ON connectors (owner, name) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS connectors_of_owner ON connectors (owner, created_at DESC);

-- One row per call through a connector, allowed or refused (AG-147): no token, header or input value.
CREATE TABLE IF NOT EXISTS connector_calls (
  id bigserial PRIMARY KEY,
  connector text NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
  at timestamptz NOT NULL DEFAULT now(),
  subject text NOT NULL,
  client text,
  tool text NOT NULL,
  target text,
  verdict text NOT NULL,
  duration_ms integer NOT NULL
);
CREATE INDEX IF NOT EXISTS connector_calls_of ON connector_calls (connector, at DESC);
