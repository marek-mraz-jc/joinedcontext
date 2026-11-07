-- Pipeline templates of a project (API/01 §42, T-3976): a pipeline's spec without what belongs
-- to the project it came from, offered in the gallery to start the next pipeline from.
CREATE TABLE IF NOT EXISTS pipeline_templates (
  id text PRIMARY KEY,
  -- The order of saving: two saves in one clock tick still list newest first.
  seq bigserial NOT NULL,
  project text NOT NULL,
  title text NOT NULL,
  spec jsonb NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS pipeline_templates_project ON pipeline_templates (project, seq);
