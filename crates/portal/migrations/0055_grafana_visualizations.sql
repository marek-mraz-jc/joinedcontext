-- A person's visualization in the project's Grafana (T-6509). Grafana keeps its database in the
-- pod, so these rows are what puts each dashboard back after a restart. No person is named here:
-- the Portal's rights decide who may see or remove a row.
CREATE TABLE IF NOT EXISTS grafana_visualizations (
  id text PRIMARY KEY,
  project text NOT NULL,
  title text NOT NULL,
  endpoint text NOT NULL,
  entity_type text NOT NULL,
  attribute text NOT NULL,
  kind text NOT NULL CHECK (kind IN ('timeseries', 'stat', 'table', 'map')),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS grafana_visualizations_project ON grafana_visualizations (project);
