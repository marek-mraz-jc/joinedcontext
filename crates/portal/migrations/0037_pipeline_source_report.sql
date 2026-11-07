-- The source-vs-space report of a snapshot pipeline (T-4892, PL-97, PL-100): the last run that
-- read its source completely, and the entities it deleted because the source no longer listed
-- them, each with the run that decided it. Ids only, never a record; removals are kept 400 days.
CREATE TABLE IF NOT EXISTS pipeline_source_complete (
  project  text        NOT NULL,
  pipeline text        NOT NULL,
  run      text        NOT NULL,
  at       timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, pipeline)
);

CREATE TABLE IF NOT EXISTS pipeline_source_removed (
  project   text        NOT NULL,
  space     text        NOT NULL,
  entity_id text        NOT NULL,
  pipeline  text        NOT NULL,
  run       text        NOT NULL,
  at        timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, space, entity_id)
);

CREATE INDEX IF NOT EXISTS pipeline_source_removed_by_pipeline
  ON pipeline_source_removed (project, pipeline, at DESC);
CREATE INDEX IF NOT EXISTS pipeline_source_removed_at
  ON pipeline_source_removed (project, space, at DESC);
