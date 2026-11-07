-- The phase and conditions the reconciling replica last gave each stream pipeline (T-2976).
-- A replica that serves without reconciling answers from here instead of calling every pipeline
-- Pending during a rollout; a row older than the Portal's freshness window is not trusted.
CREATE TABLE IF NOT EXISTS pipeline_status (
  project    text        NOT NULL,
  pipeline   text        NOT NULL,
  status     jsonb       NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, pipeline)
);
