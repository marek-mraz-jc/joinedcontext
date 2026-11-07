-- Saved data views of a space (API/01 §30, ADR-N-042 §3.2, T-3104): the Portal's own record of
-- how one entity type is looked at. The rows are the space's entities; nothing here is data.
CREATE TABLE IF NOT EXISTS data_views (
  id text PRIMARY KEY,
  project text NOT NULL,
  space text NOT NULL,
  entity_type text NOT NULL,
  kind text NOT NULL,
  mode text NOT NULL,
  title text NOT NULL,
  owner_subject text NOT NULL,
  owner_name text NOT NULL,
  config jsonb NOT NULL,
  version bigint NOT NULL DEFAULT 1,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS data_views_space ON data_views (project, space, owner_subject);
